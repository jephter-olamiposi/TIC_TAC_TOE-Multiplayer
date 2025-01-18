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
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};
use uuid::Uuid;

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
    pub draw: bool,
    pub last_activity: SystemTime,
    pub players: Vec<Player>, // Track players
}

impl Default for Game {
    fn default() -> Self {
        Game {
            board: [[None; 3]; 3],
            current_turn: Player::X,
            game_over: false,
            draw: false,
            last_activity: SystemTime::now(),
            players: Vec::new(),
        }
    }
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
        } else if self.is_full() {
            self.game_over = true;
            self.draw = true;
        } else {
            self.current_turn = match self.current_turn {
                Player::X => Player::O,
                Player::O => Player::X,
            };
        }

        self.last_activity = SystemTime::now();
        Ok(())
    }

    fn check_winner(&self) -> Option<Player> {
        for i in 0..3 {
            if self.board[i][0] == self.board[i][1] && self.board[i][1] == self.board[i][2] {
                if let Some(player) = self.board[i][0] {
                    return Some(player);
                }
            }
            if self.board[0][i] == self.board[1][i] && self.board[1][i] == self.board[2][i] {
                if let Some(player) = self.board[0][i] {
                    return Some(player);
                }
            }
        }

        if self.board[0][0] == self.board[1][1] && self.board[1][1] == self.board[2][2] {
            if let Some(player) = self.board[0][0] {
                return Some(player);
            }
        }
        if self.board[0][2] == self.board[1][1] && self.board[1][1] == self.board[2][0] {
            if let Some(player) = self.board[0][2] {
                return Some(player);
            }
        }

        None
    }
}

#[derive(Clone)]
pub struct AppState {
    games: Arc<RwLock<HashMap<String, Game>>>,
    tx: broadcast::Sender<(String, Game)>,
}

#[derive(Serialize, Deserialize)]
pub struct MoveRequest {
    game_id: String,
    x: usize,
    y: usize,
}

async fn get_state_handler(
    State(state): State<Arc<AppState>>,
    Json(game_id): Json<String>,
) -> Json<Game> {
    let games = state.games.read().await;
    let game = games.get(&game_id).cloned().unwrap_or_default();
    Json(game)
}

async fn make_move_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MoveRequest>,
) -> Json<Result<String, String>> {
    let mut games = state.games.write().await;
    let game = games.entry(req.game_id.clone()).or_default();
    let result = game.make_move(req.x, req.y);

    if result.is_ok() {
        let _ = state.tx.send((req.game_id.clone(), game.clone()));
    }

    Json(result.map(|_| "Move made".to_string()))
}

async fn reset_handler(
    State(state): State<Arc<AppState>>,
    Json(game_id): Json<String>,
) -> Json<String> {
    let mut games = state.games.write().await;
    let game = games.entry(game_id.clone()).or_default();
    game.reset();
    let _ = state.tx.send((game_id, game.clone()));
    Json("Game reset".to_string())
}

#[axum::debug_handler]
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.tx.subscribe();

    while let Ok((game_id, game)) = rx.recv().await {
        if socket
            .send(Message::Text(
                serde_json::to_string(&(game_id, game)).unwrap().into(),
            ))
            .await
            .is_err()
        {
            error!("WebSocket disconnected");
            break;
        }
    }
}

async fn cleanup_inactive_games(app_state: Arc<AppState>) {
    let timeout = Duration::from_secs(1800); // 30 minutes
    loop {
        {
            let mut games = app_state.games.write().await;
            games.retain(|_, game| game.last_activity.elapsed().unwrap_or(timeout) < timeout);
        }
        tokio::time::sleep(Duration::from_secs(60)).await; // Run every minute
    }
}

async fn create_game_handler(State(state): State<Arc<AppState>>) -> Json<String> {
    let game_id = Uuid::new_v4().to_string();
    let mut games = state.games.write().await;
    games.insert(game_id.clone(), Game::default());
    Json(game_id)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let (tx, _) = broadcast::channel(100);
    let app_state = Arc::new(AppState {
        games: Arc::new(RwLock::new(HashMap::new())),
        tx,
    });

    let app = Router::new()
        .route("/create_game", post(create_game_handler))
        .route("/state", post(get_state_handler))
        .route("/make_move", post(make_move_handler))
        .route("/reset", post(reset_handler))
        .route("/ws", get(ws_handler))
        .with_state(Arc::clone(&app_state));

    tokio::spawn(async move {
        cleanup_inactive_games(Arc::clone(&app_state)).await;
    });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    info!("Listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}
