use axum::{
    extract::{State, WebSocketUpgrade},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use anyhow::Result;

use std::env;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    pub scores: HashMap<Player, u32>,
    pub player_names: HashMap<Player, String>,
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
            player_names: HashMap::new(),
            scores: [(Player::X, 0), (Player::O, 0)].into_iter().collect(),
        }
    }
}

impl Game {
    // Resets the game to its default state
    pub fn reset(&mut self) {
        let players = self.players.clone();
        let names = self.player_names.clone();
        let scores = self.scores.clone();
        let previous_first = self.current_turn;

        // Instead of full default, create new Game manually so we can set current_turn properly
        let mut new_game = Game::default();

        new_game.players = players;
        new_game.player_names = names;
        new_game.scores = scores;

        // ✅ Alternate who plays first
        new_game.current_turn = match previous_first {
            Player::X => Player::O,
            Player::O => Player::X,
        };

        *self = new_game;

        debug!(
            "Game reset. New first player: {:?}, Names: {:?}, Scores: {:?}",
            self.current_turn, self.player_names, self.scores
        );
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
            *self.scores.entry(player).or_insert(0) += 1; // ✅ Increment score
            debug!("Game over: {:?} wins. Score updated.", player);
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

// WebSocket handler to manage real-time updates

#[axum::debug_handler]

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    info!("🔗 WebSocket connection attempt received!");

    ws.on_upgrade(move |socket| async move {
        info!("✅ WebSocket upgrade successful.");
        if let Err(e) = handle_socket(socket, state).await {
            error!("❌ WebSocket processing failed: {}", e);
        }
    })
}

// Handles WebSocket connections for broadcasting game updates
async fn handle_socket(
    mut socket: axum::extract::ws::WebSocket,
    state: Arc<AppState>,
) -> Result<()> {
    let mut rx = state.tx.subscribe();
    let mut subscribed_game_id: Option<String> = None;

    info!("✅ WebSocket connection established.");

    loop {
        info!("🕵️ Waiting for WebSocket message...");

        tokio::select! {
            Some(Ok(msg)) = socket.recv() => {
                match msg {
                    axum::extract::ws::Message::Text(text) => {
                        info!("📩 Received WebSocket message: {}", text);

                        let parsed: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(json) => json,
                            Err(_) => {
                                error!("❌ Failed to parse WebSocket message: {}", text);
                                continue;
                            }
                        };

                        match parsed["type"].as_str() {
                            Some("JOIN_GAME") => {
                                info!("✅ Processing JOIN_GAME message.");
                                handle_join_game(&parsed, &state, &mut socket).await?;
                                subscribed_game_id = parsed["game_id"].as_str().map(|s| s.to_string());
                            }
                            Some("MAKE_MOVE") => {
                                info!("✅ Processing MAKE_MOVE message.");
                                handle_make_move(&parsed, &state, &mut socket).await?;
                                if subscribed_game_id.is_none() {
                                    subscribed_game_id = parsed["game_id"].as_str().map(|s| s.to_string());
                                }
                            }
                            Some("RESET_GAME") => {
                                info!("✅ Processing RESET_GAME message.");
                                handle_reset_game(&parsed, &state).await?;
                            }
                            _ => error!("⚠️ Unknown message type received: {:?}", parsed["type"]),
                        }
                    }
                    axum::extract::ws::Message::Ping(data) => {
                        info!("📩 Received Ping: {:?}", data);
                        socket.send(axum::extract::ws::Message::Pong(data)).await?;
                    }
                    axum::extract::ws::Message::Pong(data) => {
                        info!("📩 Received Pong: {:?}", data);
                    }
                    axum::extract::ws::Message::Close(reason) => {
                        info!("❌ WebSocket closed: {:?}", reason);
                        break;
                    }
                    _ => error!("⚠️ Received unexpected WebSocket message."),
                }
            }

            Ok((game_id, game)) = rx.recv() => {
                info!("📩 WebSocket received game update for game_id={}", game_id);
                if let Some(ref subscribed_id) = subscribed_game_id {
                    if *subscribed_id == game_id {
                        let game_update = json!({
                            "type": "UPDATE_STATE",
                            "game_id": game_id,
                            "game": game
                        });

                        info!("📤 Sending WebSocket update: {}", game_update);
                        if let Err(e) = socket
                            .send(axum::extract::ws::Message::Text(game_update.to_string().into()))
                            .await
                        {
                            error!("❌ Failed to send game update: {}", e);
                        }
                    }
                }
            }
            else => {
                error!("❌ WebSocket connection lost unexpectedly.");
                break;
            }
        }
    }

    error!("❌ WebSocket closed. Cleaning up.");
    Ok(())
}

async fn handle_join_game(
    parsed: &serde_json::Value,
    state: &Arc<AppState>,
    socket: &mut axum::extract::ws::WebSocket,
) -> Result<()> {
    let game_id = parsed["game_id"].as_str().unwrap_or("").to_string();
    let name = parsed["name"].as_str().unwrap_or("Anonymous").to_string();

    info!(
        "📥 Received JOIN_GAME request - Game ID: {}, Name: {}",
        game_id, name
    );

    let mut games = state.games.write().await;
    let game = games.entry(game_id.clone()).or_insert_with(|| {
        info!("🆕 Creating new game with ID: {}", game_id);
        Game::default()
    });

    if game.players.len() >= 2 {
        error!("❌ Join request rejected: Game {} is full", game_id);
        let error_message = json!({ "type": "ERROR", "message": "Game is full" });
        socket
            .send(axum::extract::ws::Message::Text(
                error_message.to_string().into(),
            ))
            .await?;
        return Ok(());
    }

    let assigned_player = if game.players.contains(&Player::X) {
        Player::O
    } else {
        Player::X
    };

    game.players.push(assigned_player);
    game.player_names.insert(assigned_player, name.clone());
    game.scores.entry(assigned_player).or_insert(0);

    let _ = state.tx.send((game_id.clone(), game.clone()));

    info!(
        "✅ Player {:?} ({}) successfully joined game {}",
        assigned_player, name, game_id
    );

    let join_success_msg = json!({
        "type": "JOIN_SUCCESS",
        "player": assigned_player,
        "game_id": game_id,
        "name": name,
        "scores": game.scores,
        "names": game.player_names
    });

    socket
        .send(axum::extract::ws::Message::Text(
            join_success_msg.to_string().into(),
        ))
        .await?;

    let game_update = json!({
        "type": "UPDATE_STATE",
        "game_id": game_id,
        "game": game
    });

    socket
        .send(axum::extract::ws::Message::Text(
            game_update.to_string().into(),
        ))
        .await?;

    Ok(())
}

async fn handle_make_move(
    parsed: &serde_json::Value,
    state: &Arc<AppState>,
    socket: &mut axum::extract::ws::WebSocket,
) -> Result<()> {
    let game_id = parsed["game_id"].as_str().unwrap_or("").to_string();
    let x = parsed["x"].as_u64().unwrap_or(100) as usize; // Default to invalid move
    let y = parsed["y"].as_u64().unwrap_or(100) as usize; // Default invalid move

    let player = match parsed["player"].as_str() {
        Some("X") => Player::X,
        Some("O") => Player::O,
        _ => {
            error!(
                "❌ Invalid player received in MOVE request: {:?}",
                parsed["player"]
            );
            let error_msg = json!({ "type": "MOVE_FAILED", "message": "Invalid player" });
            socket
                .send(axum::extract::ws::Message::Text(
                    error_msg.to_string().into(),
                ))
                .await?;
            return Ok(());
        }
    };

    info!(
        "📥 MOVE request received - Game ID: {}, Player: {:?}, Position: ({}, {})",
        game_id, player, x, y
    );

    if x >= 3 || y >= 3 {
        error!("❌ Invalid MOVE request: Out of bounds - ({}, {})", x, y);
        let error_msg = json!({ "type": "MOVE_FAILED", "message": "Coordinates out of bounds" });
        socket
            .send(axum::extract::ws::Message::Text(
                error_msg.to_string().into(),
            ))
            .await?;
        return Ok(());
    }

    let mut games = state.games.write().await;
    if let Some(game) = games.get_mut(&game_id) {
        if !game.players.contains(&player) {
            error!(
                "❌ Player {:?} is not in game {}. Move rejected.",
                player, game_id
            );
            let error_msg = json!({ "type": "MOVE_FAILED", "message": "Player not in game" });
            socket
                .send(axum::extract::ws::Message::Text(
                    error_msg.to_string().into(),
                ))
                .await?;
            return Ok(());
        }

        match game.make_move(player, x, y) {
            Ok(_) => {
                info!(
                    "✅ Move applied: {:?} at ({}, {}) in game {}",
                    player, x, y, game_id
                );
                let update_msg = json!({
                    "type": "UPDATE_STATE",
                    "game": game
                });
                let _ = state.tx.send((game_id.clone(), game.clone()));
                socket
                    .send(axum::extract::ws::Message::Text(
                        update_msg.to_string().into(),
                    ))
                    .await?;
            }
            Err(err) => {
                error!("❌ Move failed: {}", err);
                let error_msg = json!({ "type": "MOVE_FAILED", "message": err });
                socket
                    .send(axum::extract::ws::Message::Text(
                        error_msg.to_string().into(),
                    ))
                    .await?;
            }
        }
    } else {
        error!("❌ Game ID {} not found.", game_id);
        let error_msg = json!({ "type": "MOVE_FAILED", "message": "Game ID not found." });
        socket
            .send(axum::extract::ws::Message::Text(
                error_msg.to_string().into(),
            ))
            .await?;
    }

    Ok(())
}

async fn handle_reset_game(parsed: &serde_json::Value, state: &Arc<AppState>) -> Result<()> {
    let game_id = parsed["game_id"].as_str().unwrap_or("").to_string();
    info!("📥 Received RESET_GAME request - Game ID: {}", game_id);

    let mut games = state.games.write().await;
    if let Some(game) = games.get_mut(&game_id) {
        game.reset();
        let _ = state.tx.send((game_id.clone(), game.clone()));
        info!("✅ Game {} has been reset.", game_id);
    } else {
        error!("❌ Game ID {} not found for reset.", game_id);
    }

    Ok(())
}

// Periodically cleans up inactive games
async fn cleanup_inactive_games(app_state: Arc<AppState>) {
    let timeout = Duration::from_secs(1800); // 30 minutes

    loop {
        tokio::time::sleep(Duration::from_secs(600)).await; // Run every 10 min

        let mut games = app_state.games.write().await;
        let before_cleanup = games.len();

        games.retain(|_, game| game.last_activity.elapsed().unwrap_or(timeout) < timeout);

        if before_cleanup != games.len() {
            info!("Cleaned up inactive games. Remaining: {}", games.len());
        }
    }
}

// Handler to create a new game

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .init();

    let (tx, _) = broadcast::channel(500);
    let app_state = Arc::new(AppState {
        games: Arc::new(RwLock::new(HashMap::new())),
        tx,
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(Arc::clone(&app_state));

    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);

    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");

    info!("Server is running on {}", listener.local_addr().unwrap());

    tokio::spawn(cleanup_inactive_games(Arc::clone(&app_state)));
    if let Err(e) = axum::serve(listener, app.into_make_service()).await {
        error!("❌ Server error: {}", e);
    }
}
