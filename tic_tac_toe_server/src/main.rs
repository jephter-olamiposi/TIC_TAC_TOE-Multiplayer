use axum::{
    extract::{Json, State, WebSocketUpgrade},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use std::{
    collections::HashMap,
    env,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Player {
    X,
    O,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub board: [[Option<Player>; 3]; 3], // Represents the game board (3x3 grid) with optional Player symbols
    pub current_turn: Player,            // Tracks whose turn it is (Player::X or Player::O)
    pub game_over: bool,                 // Indicates if the game is over
    pub draw: bool,                      // Indicates if the game ended in a draw
    pub last_activity: SystemTime,       // Timestamp of the last game activity
    pub players: Vec<Player>,            // List of players who joined the game
}

impl Default for Game {
    fn default() -> Self {
        Game {
            board: [[None; 3]; 3],
            current_turn: Player::X, // Default starting turn is Player::X
            game_over: false,
            draw: false,
            last_activity: SystemTime::now(),
            players: Vec::new(),
        }
    }
}

impl Game {
    // Resets the game to its default state
    fn reset(&mut self) {
        *self = Game::default();
        debug!("Game reset to default state.");
    }

    // Handles making a move on the board
    fn make_move(&mut self, player: Player, x: usize, y: usize) -> Result<(), String> {
        if self.game_over {
            debug!("Move rejected: Game is already over.");
            return Err("Game is over!".to_string());
        }
        if self.current_turn != player {
            debug!("Move rejected: Not {:?}'s turn.", player);
            return Err(format!("It's not {:?}'s turn.", player));
        }
        if x >= 3 || y >= 3 {
            debug!("Move rejected: Coordinates out of bounds.");
            return Err("Out of bounds".to_string());
        }
        if self.board[x][y].is_some() {
            debug!("Move rejected: Cell already taken.");
            return Err("Cell already taken".to_string());
        }

        self.board[x][y] = Some(player);

        if self.check_winner().is_some() {
            self.game_over = true;
            debug!("Game over: {:?} wins.", player);
        } else if self.is_full() {
            self.game_over = true;
            self.draw = true;
            debug!("Game over: It's a draw.");
        } else {
            self.current_turn = match self.current_turn {
                Player::X => Player::O,
                Player::O => Player::X,
            };
            debug!("Turn switched: Now it's {:?}'s turn.", self.current_turn);
        }

        self.last_activity = SystemTime::now();
        Ok(())
    }

    // Checks if there is a winner on the board
    fn check_winner(&self) -> Option<Player> {
        for i in 0..3 {
            // Check rows and columns for a winner
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

        // Check diagonals for a winner
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

    // Checks if the board is full (no more moves can be made)
    fn is_full(&self) -> bool {
        self.board
            .iter()
            .all(|row| row.iter().all(|&cell| cell.is_some()))
    }
}

#[derive(Clone)]
pub struct AppState {
    games: Arc<RwLock<HashMap<String, Game>>>, // Shared state of all games
    tx: broadcast::Sender<(String, Game)>,     // Broadcast channel for game updates
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinGameRequest {
    pub game_id: String,
    pub player: Option<Player>,
}

#[derive(Debug, Serialize, Deserialize)] // ✅ Add Debug
pub struct MoveRequest {
    pub game_id: String, // ID of the game where the move is made
    pub player: Player,  // Player making the move
    pub x: usize,        // Row of the move
    pub y: usize,        // Column of the move
}

async fn join_game_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinGameRequest>,
) -> Json<Result<Player, String>> {
    // ✅ Return Player enum
    debug!("Received join request: {:?}", req);

    let mut games = state.games.write().await;
    let game = games
        .entry(req.game_id.clone())
        .or_insert_with(Game::default);

    if game.players.len() >= 2 {
        error!("Join request rejected: Game {} is full", req.game_id);
        return Json(Err("Game is already full.".to_string()));
    }

    let assigned_player = match req.player {
        Some(requested) if !game.players.contains(&requested) => requested,
        Some(_) => {
            error!(
                "Join request rejected: Player {:?} already taken in game {}",
                req.player, req.game_id
            );
            return Json(Err("Player already taken.".to_string()));
        }
        None => {
            if game.players.contains(&Player::X) {
                Player::O
            } else {
                Player::X
            }
        }
    };

    game.players.push(assigned_player);
    let _ = state.tx.send((req.game_id.clone(), game.clone()));

    info!(
        "Player {:?} successfully joined game {}",
        assigned_player, req.game_id
    );

    Json(Ok(assigned_player)) // ✅ Return `Player` enum directly
}

// Handler to fetch the state of a game
async fn get_state_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>, // ✅ Expect JSON input
) -> Json<serde_json::Value> {
    debug!("📥 Received get_state request: {:?}", payload);

    let game_id = match payload.get("game_id").and_then(|id| id.as_str()) {
        Some(id) => id,
        None => {
            error!("❌ Invalid request: Missing or incorrect 'game_id'");
            return Json(json!({ "Err": "Invalid request: Missing game_id" }));
        }
    };

    let games = state.games.read().await;
    if let Some(game) = games.get(game_id) {
        debug!("✅ Returning game state for {}", game_id);
        Json(json!(game))
    } else {
        error!("❌ Game {} not found", game_id);
        Json(json!({ "Err": "Game not found" }))
    }
}

// Handler to make a move in a game
async fn make_move_handler(
    State(state): State<Arc<AppState>>,
    req: Result<Json<MoveRequest>, axum::extract::rejection::JsonRejection>,
) -> Json<Result<String, String>> {
    // Handle invalid JSON requests
    let req = match req {
        Ok(req) => req.0, // Extract the MoveRequest
        Err(_) => {
            error!("Invalid JSON format received for make_move.");
            return Json(Err("Invalid JSON format".to_string()));
        }
    };

    debug!("Received make_move request: {:?}", req);

    let mut games = state.games.write().await;
    let game = games.entry(req.game_id.clone()).or_default();
    let result = game.make_move(req.player, req.x, req.y);

    if result.is_ok() {
        let _ = state.tx.send((req.game_id.clone(), game.clone())); // Broadcast the new game state
        info!(
            "Move made: Game ID: {}, Player: {:?}, Position: ({}, {})",
            req.game_id, req.player, req.x, req.y
        );
    } else {
        error!(
            "Move rejected: Game ID: {}, Player: {:?}, Position: ({}, {}). Reason: {:?}",
            req.game_id, req.player, req.x, req.y, result
        );
    }

    Json(result.map(|_| "Move made".to_string()))
}

// Handler to reset a game
async fn reset_handler(
    State(state): State<Arc<AppState>>,
    Json(game_id): Json<String>,
) -> Json<String> {
    let mut games = state.games.write().await;
    let game = games.entry(game_id.clone()).or_default();
    game.reset();
    let _ = state.tx.send((game_id.clone(), game.clone()));
    info!("Game {} has been reset.", game_id);
    Json("Game reset".to_string())
}

// WebSocket handler to manage real-time updates
#[axum::debug_handler]
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

// Handles WebSocket connections for broadcasting game updates
async fn handle_socket(mut socket: axum::extract::ws::WebSocket, state: Arc<AppState>) {
    let mut rx = state.tx.subscribe();

    // ✅ Fetch all current game states and send them to the new client immediately
    {
        let games = state.games.read().await;
        for (game_id, game) in games.iter() {
            let _ = socket
                .send(axum::extract::ws::Message::Text(
                    serde_json::to_string(&(game_id.clone(), game.clone()))
                        .unwrap()
                        .into(),
                ))
                .await;
        }
    } // 🔴 Unlock read access here before listening for updates

    // ✅ Listen for real-time updates and broadcast changes
    while let Ok((game_id, game)) = rx.recv().await {
        if socket
            .send(axum::extract::ws::Message::Text(
                serde_json::to_string(&(game_id, game)).unwrap().into(),
            ))
            .await
            .is_err()
        {
            error!("WebSocket client disconnected. Stopping updates.");
            break;
        }
    }
}

// Periodically cleans up inactive games
async fn cleanup_inactive_games(app_state: Arc<AppState>) {
    let timeout = Duration::from_secs(1800); // 30 minutes

    loop {
        {
            let mut games = app_state.games.write().await;
            let before_cleanup = games.len();
            for (game_id, game) in games.iter_mut() {
                if game.last_activity.elapsed().unwrap_or(timeout) >= timeout {
                    game.reset(); // Reset instead of deleting
                    let _ = app_state.tx.send((game_id.clone(), game.clone()));
                    info!("Inactive game {} has been reset.", game_id);
                }
            }
            let after_cleanup = games.len();
            if before_cleanup != after_cleanup {
                info!(
                    "Cleaned up inactive games. Remaining games: {}",
                    after_cleanup
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

// Handler to create a new game
async fn create_game_handler(State(state): State<Arc<AppState>>) -> Json<String> {
    let game_id = Uuid::new_v4().to_string();
    let mut games = state.games.write().await;
    games.insert(game_id.clone(), Game::default());
    info!("New game created with ID: {}", game_id);
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

    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .route("/create_game", post(create_game_handler))
        .route("/state", post(get_state_handler))
        .route("/make_move", post(make_move_handler))
        .route("/reset", post(reset_handler))
        .route("/join_game", post(join_game_handler))
        .route("/ws", get(ws_handler))
        .layer(cors)
        .with_state(Arc::clone(&app_state));

    // Use the PORT environment variable provided by Render
    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);

    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");

    info!("Server is running on {}", listener.local_addr().unwrap());

    tokio::spawn(cleanup_inactive_games(Arc::clone(&app_state)));
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}
