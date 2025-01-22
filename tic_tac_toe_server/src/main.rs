use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Json, State, WebSocketUpgrade},
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, sync::Arc, time::SystemTime};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc, Mutex, RwLock},
};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, info};
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
    pub players: Vec<Player>,
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
        debug!("Game reset to default state.");
    }

    fn make_move(&mut self, player: Player, x: usize, y: usize) -> Result<(), String> {
        debug!("Processing move for player {:?} at ({}, {})", player, x, y);
        if self.game_over {
            return Err("Game is over!".to_string());
        }
        if self.current_turn != player {
            return Err(format!("It's not {:?}'s turn.", player));
        }
        if x >= 3 || y >= 3 {
            return Err("Out of bounds".to_string());
        }
        if self.board[x][y].is_some() {
            return Err("Cell already taken".to_string());
        }

        self.board[x][y] = Some(player);

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
                return self.board[i][0];
            }
            if self.board[0][i] == self.board[1][i] && self.board[1][i] == self.board[2][i] {
                return self.board[0][i];
            }
        }

        if self.board[0][0] == self.board[1][1] && self.board[1][1] == self.board[2][2] {
            return self.board[0][0];
        }
        if self.board[0][2] == self.board[1][1] && self.board[1][1] == self.board[2][0] {
            return self.board[0][2];
        }

        None
    }

    fn is_full(&self) -> bool {
        self.board
            .iter()
            .all(|row| row.iter().all(|&cell| cell.is_some()))
    }
}

#[derive(Clone)]
pub struct AppState {
    games: Arc<RwLock<HashMap<String, Game>>>,
    tx: broadcast::Sender<(String, Game)>,
    websocket_clients: Arc<Mutex<HashMap<String, Vec<mpsc::Sender<Message>>>>>,
}

#[derive(Serialize, Deserialize)]
pub struct MoveRequest {
    game_id: String,
    player: Player,
    x: usize,
    y: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinGameRequest {
    game_id: String,
    player: Option<Player>,
}

async fn create_game_handler(State(state): State<Arc<AppState>>) -> Json<String> {
    let game_id = Uuid::new_v4().to_string();
    let mut games = state.games.write().await;
    games.insert(game_id.clone(), Game::default());
    Json(game_id)
}

async fn get_state_handler(
    State(state): State<Arc<AppState>>,
    Json(game_id): Json<String>,
) -> Json<Game> {
    let games = state.games.read().await;
    if let Some(game) = games.get(&game_id) {
        Json(game.clone())
    } else {
        Json(Game::default())
    }
}

async fn join_game_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinGameRequest>,
) -> Json<Result<Player, String>> {
    let mut games = state.games.write().await;
    let game = games
        .entry(req.game_id.clone())
        .or_insert_with(Game::default);

    if game.players.len() >= 2 {
        return Json(Err("Game is full".to_string()));
    }

    let assigned_player = if let Some(requested_player) = req.player {
        if game.players.contains(&requested_player) {
            return Json(Err(format!(
                "Player {:?} is already taken",
                requested_player
            )));
        }
        requested_player
    } else {
        if game.players.contains(&Player::X) {
            Player::O
        } else {
            Player::X
        }
    };

    game.players.push(assigned_player);
    Json(Ok(assigned_player))
}

// Example: Use `tx` to broadcast game updates
async fn make_move_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MoveRequest>,
) -> Json<Result<String, String>> {
    let mut games = state.games.write().await;
    if let Some(game) = games.get_mut(&req.game_id) {
        let result = game.make_move(req.player, req.x, req.y);
        if result.is_ok() {
            let message = serde_json::to_string(&game).unwrap();

            // Use broadcast sender to notify subscribers
            let _ = state.tx.send((req.game_id.clone(), game.clone()));

            let websocket_clients = state.websocket_clients.lock().await;
            if let Some(clients) = websocket_clients.get(&req.game_id) {
                for client in clients {
                    if let Err(e) = client.send(Message::Text(message.clone().into())).await {
                        tracing::warn!("Failed to send WebSocket message: {}", e);
                    }
                }
            }
        }
        return Json(result.map(|_| "Move made".to_string()));
    }
    Json(Err("Game not found".to_string()))
}

async fn reset_handler(
    State(state): State<Arc<AppState>>,
    Json(game_id): Json<String>,
) -> Json<String> {
    let mut games = state.games.write().await;
    if let Some(game) = games.get_mut(&game_id) {
        game.reset();
        let message = serde_json::to_string(&game).unwrap();
        let websocket_clients = state.websocket_clients.lock().await;
        if let Some(clients) = websocket_clients.get(&game_id) {
            for client in clients {
                if let Err(e) = client.send(Message::Text(message.clone().into())).await {
                    tracing::warn!("Failed to send WebSocket message: {}", e);
                }
            }
        }
    }
    Json("Game reset".to_string())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    let game_id = params.get("game_id").cloned().unwrap_or_default();
    ws.on_upgrade(move |socket| handle_socket(socket, state, game_id))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>, game_id: String) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel(32);

    // Register the WebSocket client
    {
        let mut websocket_clients = state.websocket_clients.lock().await;
        websocket_clients
            .entry(game_id.clone()) // Clone game_id here
            .or_default()
            .push(tx);
    }

    // Clone game_id for each async block
    let cleanup_game_id = game_id.clone();
    tokio::spawn({
        let state = state.clone();
        async move {
            while let Some(msg) = receiver.next().await {
                match msg {
                    Ok(Message::Close(_)) => {
                        tracing::info!("Client disconnected from game: {}", cleanup_game_id);
                        break;
                    }
                    Ok(_) => {
                        // Handle other message types if needed
                    }
                    Err(e) => {
                        tracing::warn!("WebSocket error: {}", e);
                        break;
                    }
                }
            }

            // Remove client on disconnect
            let mut websocket_clients = state.websocket_clients.lock().await;
            if let Some(clients) = websocket_clients.get_mut(&cleanup_game_id) {
                clients.retain(|client| !client.is_closed());
            }
            tracing::info!(
                "Cleaned up disconnected client for game: {}",
                cleanup_game_id
            );
        }
    });

    // Clone game_id for use in this loop
    let send_game_id = game_id.clone();
    while let Some(message) = rx.recv().await {
        if sender.send(message).await.is_err() {
            tracing::warn!(
                "Failed to send message to client for game: {}",
                send_game_id
            );
            break;
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let (tx, _) = broadcast::channel(1000);

    let app_state = Arc::new(AppState {
        games: Arc::new(RwLock::new(HashMap::new())),
        tx: tx.clone(),
        websocket_clients: Arc::new(Mutex::new(HashMap::new())),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/create_game", post(create_game_handler))
        .route("/state", post(get_state_handler))
        .route("/join_game", post(join_game_handler))
        .route("/make_move", post(make_move_handler))
        .route("/reset", post(reset_handler))
        .route("/ws", get(ws_handler))
        .layer(cors)
        .with_state(app_state);

    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);

    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");

    info!("Server running at {}", addr);

    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}
