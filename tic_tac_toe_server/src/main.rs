use axum::{
    extract::{Json, State, WebSocketUpgrade},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info};
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

async fn join_game_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinGameRequest>,
) -> Json<Result<Player, String>> {
    info!("Received join game request: {:?}", req);
    let mut games = state.games.write().unwrap();
    let game = games
        .entry(req.game_id.clone())
        .or_insert_with(Game::default);

    if game.players.len() >= 2 {
        error!("Join game failed: Game {} is already full.", req.game_id);
        return Json(Err("Game is already full.".to_string()));
    }

    if let Some(requested_player) = req.player {
        if game.players.contains(&requested_player) {
            error!(
                "Join game failed: Player {:?} already taken in game {}.",
                requested_player, req.game_id
            );
            return Json(Err(format!(
                "Player {:?} is already taken. Choose a different symbol.",
                requested_player
            )));
        }
        game.players.push(requested_player);
        info!(
            "Player {:?} successfully joined game {} as the first player.",
            requested_player, req.game_id
        );

        // Broadcast the game state update
        if let Err(e) = state.tx.send((req.game_id.clone(), game.clone())) {
            error!(
                "Failed to broadcast updated game state after player joined: {:?}",
                e
            );
        }

        return Json(Ok(requested_player));
    }

    let assigned_player = if game.players.contains(&Player::X) {
        Player::O
    } else {
        Player::X
    };

    game.players.push(assigned_player);
    info!(
        "Player {:?} automatically assigned to game {} as the second player.",
        assigned_player, req.game_id
    );

    // Broadcast the game state update
    if let Err(e) = state.tx.send((req.game_id.clone(), game.clone())) {
        error!(
            "Failed to broadcast updated game state after player joined: {:?}",
            e
        );
    }

    Json(Ok(assigned_player))
}

async fn get_state_handler(
    State(state): State<Arc<AppState>>,
    Json(game_id): Json<String>,
) -> Json<Game> {
    info!("Fetching state for game: {}", game_id);
    let games = state.games.read().unwrap();
    let game = games.get(&game_id).cloned().unwrap_or_default();
    Json(game)
}

async fn make_move_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MoveRequest>,
) -> Json<Result<String, String>> {
    info!(
        "Received move request: game_id={}, player={:?}, position=({}, {})",
        req.game_id, req.player, req.x, req.y
    );

    let mut games = state.games.write().unwrap();
    let game = games.entry(req.game_id.clone()).or_default();
    let result = game.make_move(req.player, req.x, req.y);

    if result.is_ok() {
        info!(
            "Move made: game_id={}, player={:?}, position=({}, {})",
            req.game_id, req.player, req.x, req.y
        );

        // Broadcast the game state update
        if let Err(e) = state.tx.send((req.game_id.clone(), game.clone())) {
            error!("Failed to broadcast game update: {:?}", e);
        }
    } else {
        error!("Move failed: game_id={}, error={:?}", req.game_id, result);
    }

    Json(result.map(|_| "Move made".to_string()))
}

async fn reset_handler(
    State(state): State<Arc<AppState>>,
    Json(game_id): Json<String>,
) -> Json<String> {
    info!("Resetting game: {}", game_id);
    let mut games = state.games.write().unwrap();
    let game = games.entry(game_id.clone()).or_default();
    game.reset();
    let _ = state.tx.send((game_id.clone(), game.clone()));
    Json("Game reset".to_string())
}

#[axum::debug_handler]
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    info!("WebSocket upgrade request received");
    ws.on_upgrade(|socket| handle_socket(socket, state))
}
async fn handle_socket(mut socket: axum::extract::ws::WebSocket, state: Arc<AppState>) {
    info!("New WebSocket connection established");
    let mut rx = state.tx.subscribe();
    let ping_interval = tokio::time::interval(Duration::from_secs(30)); // Ping every 30 seconds
    tokio::pin!(ping_interval);

    loop {
        tokio::select! {
            // Send broadcast updates to WebSocket clients
            msg = rx.recv() => {
                match msg {
                    Ok((game_id, game)) => {
                        let message = match serde_json::to_string(&(game_id, game)) {
                            Ok(msg) => msg,
                            Err(e) => {
                                error!("Failed to serialize game state: {}", e);
                                continue;
                            }
                        };

                        if socket.send(axum::extract::ws::Message::Text(message.into())).await.is_err() {
                            error!("WebSocket connection dropped.");
                            break;
                        }
                    }
                    Err(_) => {
                        error!("Failed to receive message from broadcast channel.");
                        break;
                    }
                }
            }
            // Respond to ping messages to keep the connection alive
            Some(Ok(axum::extract::ws::Message::Ping(ping))) = socket.recv() => {
                if socket.send(axum::extract::ws::Message::Pong(ping)).await.is_err() {
                    error!("Failed to send Pong.");
                    break;
                }
            }
            // Send periodic pings to the client
            _ = ping_interval.tick() => {
               if socket.send(axum::extract::ws::Message::Ping(vec![].into())).await.is_err() {

                    error!("Failed to send Ping.");
                    break;
                }
            }
            // Handle connection closure
            else => break,
        }
    }
    info!("WebSocket connection closed");
}

async fn cleanup_inactive_games(app_state: Arc<AppState>) {
    let timeout = Duration::from_secs(1800);
    loop {
        {
            let mut games = app_state.games.write().unwrap();
            let before_cleanup = games.len();
            games.retain(|_, game| game.last_activity.elapsed().unwrap_or(timeout) < timeout);
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

async fn create_game_handler(State(state): State<Arc<AppState>>) -> Json<String> {
    let game_id = Uuid::new_v4().to_string();
    info!("Creating new game with ID: {}", game_id);
    let mut games = state.games.write().unwrap();
    games.insert(game_id.clone(), Game::default());
    Json(game_id)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let (tx, _) = broadcast::channel(100); // Set the buffer size to 100 (or larger if needed)

    let app_state = Arc::new(AppState {
        games: Arc::new(RwLock::new(HashMap::new())),
        tx: tx.clone(), // Ensure a reference to `tx` is retained here
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/create_game", post(create_game_handler))
        .route("/state", post(get_state_handler))
        .route("/make_move", post(make_move_handler))
        .route("/reset", post(reset_handler))
        .route("/join_game", post(join_game_handler))
        .route("/ws", get(ws_handler))
        .layer(cors)
        .with_state(Arc::clone(&app_state));

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
