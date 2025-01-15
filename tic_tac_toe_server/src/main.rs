use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
use tower::ServiceBuilder;
use tracing_subscriber::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Player {
    X,
    O,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub board: [[Option<Player>; 3]; 3],
    pub current_turn: Player,
    pub game_over: bool,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            board: [[None; 3]; 3],
            current_turn: Player::X,
            game_over: false,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    games: Arc<Mutex<HashMap<String, Game>>>,
    tx: broadcast::Sender<(String, Game)>,
}

impl Game {
    fn reset(&mut self) {
        *self = Game::default();
    }

    fn is_full(&self) -> bool {
        self.board
            .iter()
            .all(|row| row.iter().all(|&cell| cell.is_some()))
    }

    pub fn check_winner(&self) -> Option<Player> {
        Self::check_winner_with_board(&self.board)
    }

    fn check_winner_with_board(board: &[[Option<Player>; 3]; 3]) -> Option<Player> {
        for i in 0..3 {
            if board[i][0] == board[i][1] && board[i][1] == board[i][2] {
                if let Some(player) = board[i][0] {
                    return Some(player);
                }
            }
            if board[0][i] == board[1][i] && board[1][i] == board[2][i] {
                if let Some(player) = board[0][i] {
                    return Some(player);
                }
            }
        }
        if board[0][0] == board[1][1] && board[1][1] == board[2][2] {
            if let Some(player) = board[0][0] {
                return Some(player);
            }
        }
        if board[0][2] == board[1][1] && board[1][1] == board[2][0] {
            if let Some(player) = board[0][2] {
                return Some(player);
            }
        }
        None
    }

    fn make_move(&mut self, x: usize, y: usize) -> Result<(), String> {
        if self.game_over {
            return Err("Game is over!".to_string());
        }
        if x >= 3 || y >= 3 {
            return Err("Out of bounds".to_string());
        }
        if self.board[x][y].is_some() {
            return Err("Cell already taken".to_string());
        }

        self.board[x][y] = Some(self.current_turn);

        if self.check_winner().is_some() {
            self.game_over = true;
            return Ok(());
        }

        if self.is_full() {
            self.game_over = true;
            return Ok(());
        }

        self.current_turn = match self.current_turn {
            Player::X => Player::O,
            Player::O => Player::X,
        };

        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub struct MoveRequest {
    game_id: String,
    x: usize,
    y: usize,
}

async fn get_state_handler(
    State(state): State<AppState>,
    Json(game_id): Json<String>,
) -> Json<Game> {
    let games = state.games.lock().unwrap();
    let game = games.get(&game_id).cloned().unwrap_or_default();
    Json(game)
}

async fn make_move_handler(
    State(state): State<AppState>,
    Json(req): Json<MoveRequest>,
) -> Json<Result<String, String>> {
    let mut games = state.games.lock().unwrap();
    let game = games.entry(req.game_id.clone()).or_default();
    let result = game.make_move(req.x, req.y);
    if result.is_ok() {
        let _ = state.tx.send((req.game_id.clone(), game.clone()));
    }
    Json(result.map(|_| "Move made".to_string()))
}

async fn reset_handler(State(state): State<AppState>, Json(game_id): Json<String>) -> Json<String> {
    let mut games = state.games.lock().unwrap();
    let game = games.entry(game_id.clone()).or_default();
    game.reset();
    let _ = state.tx.send((game_id, game.clone()));
    Json("Game reset".to_string())
}

#[axum::debug_handler]
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.tx.subscribe();

    while let Ok((game_id, game)) = rx.recv().await {
        if socket
            .send(Message::Text(
                serde_json::to_string(&(game_id, game)).unwrap().into(),
            ))
            .await
            .is_err()
        {
            break;
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
            }),
        )
        .init();

    let (tx, _) = broadcast::channel(100);
    let app_state = AppState {
        games: Arc::new(Mutex::new(HashMap::new())),
        tx,
    };

    let app = Router::new()
        .route("/state", post(get_state_handler))
        .route("/make_move", post(make_move_handler))
        .route("/reset", post(reset_handler))
        .route("/ws", get(ws_handler))
        .with_state(app_state)
        .layer(ServiceBuilder::new());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on {}", listener.local_addr().unwrap());
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}
