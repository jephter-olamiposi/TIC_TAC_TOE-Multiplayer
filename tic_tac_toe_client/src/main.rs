use eframe::egui;

use futures_util::stream::StreamExt;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::SinkExt;

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::net::TcpStream;
use tokio::sync::Mutex;

use tokio_tungstenite::connect_async;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;

use std::time::Instant;
use tracing::{error, info};
use tracing_subscriber::fmt;
use tungstenite::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Player {
    X,
    O,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Game {
    pub board: [[Option<Player>; 3]; 3],
    pub current_turn: Player,
    pub game_over: bool,
    pub draw: bool,
    pub players: Vec<Player>,
    pub player_names: HashMap<Player, String>, // NEW: Player::X => "Bima", Player::O => "Redwan"
    pub scores: HashMap<Player, u32>,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            board: [[None; 3]; 3],
            current_turn: Player::X,
            game_over: false,
            draw: false,
            players: Vec::new(),
            player_names: HashMap::new(),
            scores: HashMap::from([(Player::X, 0), (Player::O, 0)]),
        }
    }
}

#[derive(Clone)]
pub struct GameService {
    server_url: String,
    game: Arc<Mutex<Game>>,
    player: Arc<Mutex<Option<Player>>>,
    connected: Arc<Mutex<bool>>,
    game_id: Arc<Mutex<String>>,
    socket: Arc<Mutex<Option<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    last_ping_time: Arc<Mutex<Option<Instant>>>,

    socket_write:
        Arc<Mutex<Option<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>>,
    socket_read: Arc<Mutex<Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>>>,
    player_name: Arc<Mutex<String>>,
}

impl GameService {
    pub fn new(server_url: String) -> Self {
        let socket = Arc::new(Mutex::new(None));

        Self {
            server_url,
            game: Arc::new(Mutex::new(Game::default())),
            player: Arc::new(Mutex::new(None)),
            connected: Arc::new(Mutex::new(false)),
            socket,
            game_id: Arc::new(Mutex::new(String::new())),
            last_ping_time: Arc::new(Mutex::new(None)),
            socket_write: Arc::new(Mutex::new(None)),
            socket_read: Arc::new(Mutex::new(None)),
            player_name: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn get_game(&self) -> Arc<Mutex<Game>> {
        Arc::clone(&self.game)
    }

    pub async fn is_connected(&self) -> bool {
        let mut socket_guard = self.socket.lock().await;
        let mut socket_write_guard = self.socket_write.lock().await;

        // Check if socket and writer exist
        if socket_guard.is_some() && socket_write_guard.is_some() {
            // Try to send a ping
            if let Some(socket) = socket_guard.as_mut() {
                match socket.send(Message::Ping(vec![].into())).await {
                    Ok(_) => {
                        *self.last_ping_time.lock().await = Some(Instant::now());
                        true
                    }
                    Err(_) => {
                        // Clear socket references on error
                        *socket_guard = None;
                        *socket_write_guard = None;
                        *self.connected.lock().await = false;
                        false
                    }
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    pub async fn get_player(&self) -> Option<Player> {
        let timeout = Instant::now() + Duration::from_secs(5);

        while Instant::now() < timeout {
            if let Some(player) = self.player.try_lock().ok().and_then(|p| *p) {
                return Some(player);
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        None
    }

    pub async fn start_websocket(
        &self,
        game_id: String,
        player_name: String,
        ctx: Arc<egui::Context>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut connected = self.connected.lock().await;

        let socket_alive = self.socket_write.lock().await.is_some();
        if *connected && socket_alive {
            info!("✅ WebSocket already running.");
            return Ok(());
        }

        *connected = true;
        drop(connected);

        let websocket_url = format!(
            "{}/ws",
            self.server_url
                .replace("http://", "ws://")
                .replace("https://", "wss://")
        );

        // ✅ connect and split immediately
        let (stream, _) = connect_async(&websocket_url).await?;
        let (write, read) = stream.split();

        // ✅ store pieces where needed
        *self.socket_write.lock().await = Some(write);
        *self.socket_read.lock().await = Some(read);
        *self.player_name.lock().await = player_name.clone();

        let join_request = serde_json::json!({
            "type": "JOIN_GAME",
            "game_id": game_id,
            "name": player_name
        });

        if let Some(writer) = &mut *self.socket_write.lock().await {
            writer
                .send(Message::Text(join_request.to_string().into()))
                .await?;
        }

        let socket_read = self.socket_read.lock().await.take();
        if let Some(socket_read) = socket_read {
            let self_clone = Arc::new(self.clone());
            let ctx_clone = Arc::clone(&ctx);

            tokio::spawn(async move {
                if let Err(e) = self_clone.listen_for_messages(socket_read, ctx_clone).await {
                    error!("Error in WebSocket listener: {:?}", e);
                }
            });
        }

        Ok(())
    }

    pub async fn reconnect(
        &self,
        game_id: String,
        ctx: Arc<egui::Context>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let max_attempts = 5;

        for attempt in 1..=max_attempts {
            {
                let mut is_connected = self.connected.lock().await;
                if *is_connected {
                    return Ok(());
                }
                *is_connected = true;
            }

            let websocket_url = self
                .server_url
                .replace("http://", "ws://")
                .replace("https://", "wss://")
                + "/ws";

            match connect_async(&websocket_url).await {
                Ok((socket, _)) => {
                    info!("✅ Reconnected successfully.");
                    let (write, read) = socket.split();
                    *self.socket_write.lock().await = Some(write);
                    *self.socket_read.lock().await = Some(read);

                    let player_name = self.player_name.lock().await.clone();
                    let join_request = serde_json::json!({
                        "type": "JOIN_GAME",
                        "game_id": game_id,
                        "name": player_name
                    });

                    if let Some(writer) = &mut *self.socket_write.lock().await {
                        writer
                            .send(Message::Text(join_request.to_string().into()))
                            .await?;
                    }

                    if let Some(socket_read) = self.socket_read.lock().await.take() {
                        let ctx_clone = Arc::clone(&ctx);
                        let self_clone = Arc::new(self.clone());

                        tokio::spawn(async move {
                            if let Err(e) =
                                self_clone.listen_for_messages(socket_read, ctx_clone).await
                            {
                                error!("❌ Error after reconnect: {:?}", e);
                            }
                        });
                    }

                    return Ok(());
                }
                Err(e) => {
                    error!("❌ Reconnection attempt {} failed: {}", attempt, e);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }

        error!("❌ Reached max reconnection attempts.");
        *self.connected.lock().await = false;
        Err("Max reconnection attempts reached".into())
    }

    async fn listen_for_messages(
        &self,
        mut socket_read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        ctx: Arc<egui::Context>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        while let Some(message) = socket_read.next().await {
            match message? {
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text)?;

                    match parsed["type"].as_str() {
                        Some("JOIN_SUCCESS") => {
                            if let Some(received_game_id) = parsed["game_id"].as_str() {
                                *self.game_id.lock().await = received_game_id.to_string();
                            }

                            if let Some(player_str) = parsed["player"].as_str() {
                                let player_type = match player_str {
                                    "X" => Some(Player::X),
                                    "O" => Some(Player::O),
                                    _ => None,
                                };

                                if let Some(p) = player_type {
                                    *self.player.lock().await = Some(p);
                                    *self.connected.lock().await = true;
                                }
                            }
                        }
                        Some("UPDATE_STATE") => {
                            if let Ok(updated_game) =
                                serde_json::from_value::<Game>(parsed["game"].clone())
                            {
                                *self.game.lock().await = updated_game;
                                ctx.request_repaint();
                            }
                        }
                        _ => error!("⚠️ Unknown message type: {}", text),
                    }
                }
                _ => {}
            }
        }

        error!("❌ WebSocket connection lost.");
        *self.connected.lock().await = false;

        Ok(())
    }
    pub async fn join_game(&self, game_id: String, player_name: String, ctx: Arc<egui::Context>) {
        let result = self.start_websocket(game_id, player_name, ctx).await;
        if let Err(e) = result {
            error!("Failed to join game: {:?}", e);
        }
    }

    pub async fn make_move(
        &self,
        game_id: String,
        player: Player,
        row: usize,
        col: usize,
        ctx: Arc<egui::Context>,
    ) {
        if game_id.trim().is_empty() {
            error!("❌ Cannot make a move: Game ID is empty!");
            return;
        }

        let move_request = serde_json::json!({
            "type": "MAKE_MOVE",
            "game_id": game_id,
            "player": match player {
                Player::X => "X",
                Player::O => "O",
            },
            "x": row,
            "y": col
        });

        info!("📤 Attempting to send MOVE request...");

        // Ensure connection is active; try to reconnect if not
        if !self.is_connected().await {
            error!("🔌 WebSocket is disconnected. Trying to reconnect...");

            if let Err(e) = self.reconnect(game_id.clone(), ctx.clone()).await {
                error!("❌ Reconnection failed: {}", e);
                return;
            }

            // After reconnecting, let the server catch up (OPTIONAL delay)
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        // Attempt to send the move
        match self.socket_write.lock().await.as_mut() {
            Some(writer) => {
                if let Err(e) = writer
                    .send(Message::Text(move_request.to_string().into()))
                    .await
                {
                    error!("❌ Failed to send MOVE request: {}", e);
                } else {
                    info!(
                        "✅ MOVE request sent: Player {:?} -> ({}, {})",
                        player, row, col
                    );
                }
            }
            None => {
                error!("❌ No active WebSocket writer. Cannot send move.");
            }
        }
    }

    pub async fn reset_game(&self) {
        let game_id = self.game_id.lock().await.clone();

        if !self.is_connected().await {
            error!("❌ No active WebSocket connection. Attempting to reconnect before reset...");

            let ctx = Arc::new(egui::Context::default());

            if let Err(e) = self.reconnect(game_id.clone(), ctx).await {
                error!("❌ Failed to reconnect before reset: {}", e);
                return;
            }
        }

        let reset_request = serde_json::json!({
            "type": "RESET_GAME",
            "game_id": game_id
        });

        let mut socket_write_guard = self.socket_write.lock().await;
        if let Some(writer) = socket_write_guard.as_mut() {
            if let Err(e) = writer
                .send(Message::Text(reset_request.to_string().into()))
                .await
            {
                error!("❌ Failed to send RESET_GAME request: {}", e);
            } else {
                info!("✅ RESET_GAME request sent successfully");
            }
        } else {
            error!("❌ WebSocket writer unavailable");
        }
    }
}

#[derive(Clone)]
pub struct GameApp {
    game_service: Arc<GameService>,
    game_id: Arc<Mutex<String>>,
    input_game_id: String,
    input_player_name: String, // ✅ NEW: player's name input

    joined: Arc<Mutex<bool>>,
    error_message: Option<String>,
    cached_player: Arc<Mutex<Option<Player>>>,
}

impl Default for GameApp {
    fn default() -> Self {
        Self {
            game_service: Arc::new(GameService::new("http://localhost:3000".to_string())),
            game_id: Arc::new(Mutex::new(String::new())),
            input_game_id: String::new(),
            input_player_name: String::new(),
            joined: Arc::new(Mutex::new(false)),
            error_message: None,
            cached_player: Arc::new(Mutex::new(None)),
        }
    }
}

impl eframe::App for GameApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let game_service = Arc::clone(&self.game_service);
        let cached_player = Arc::clone(&self.cached_player);
        let joined_state = Arc::clone(&self.joined);

        tokio::spawn(async move {
            if let Some(player) = game_service.get_player().await {
                if let Ok(mut cached) = cached_player.try_lock() {
                    *cached = Some(player);
                }
            }
        });

        let joined = joined_state.try_lock().map(|guard| *guard).unwrap_or(false);

        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        egui::CentralPanel::default().show(ctx, |ui| {
            self.handle_game_ui(ui, &Arc::new(ctx.clone()), joined);
        });
    }
}

impl GameApp {
    fn handle_game_ui(&mut self, ui: &mut egui::Ui, ctx_arc: &Arc<egui::Context>, joined: bool) {
        ui.vertical_centered(|ui| {
            ui.group(|ui| {
                ui.set_width(400.0);
                ui.set_height(500.0);

                if !joined {
                    ui.label("Your Name:");
                    ui.add_space(3.0);
                    ui.text_edit_singleline(&mut self.input_player_name);
                }

                ui.add_space(10.0);
                ui.label("Game ID:");
                ui.add_space(3.0);
                ui.text_edit_singleline(&mut self.input_game_id);

                ui.add_space(10.0);

                let can_join = !self.input_game_id.trim().is_empty()
                    && !self.input_player_name.trim().is_empty();
                if ui
                    .add_enabled(
                        can_join,
                        egui::Button::new("Join Game").min_size(egui::vec2(100.0, 30.0)),
                    )
                    .clicked()
                {
                    let ctx_clone = Arc::clone(ctx_arc);
                    let game_service_clone = Arc::clone(&self.game_service);
                    let input_game_id = self.input_game_id.clone();
                    let player_name = self.input_player_name.clone();
                    let joined_state = Arc::clone(&self.joined);
                    let game_id_lock = Arc::clone(&self.game_id);

                    tokio::spawn(async move {
                        let id = input_game_id.clone();
                        game_service_clone
                            .join_game(input_game_id, player_name, ctx_clone)
                            .await;

                        if let Ok(mut joined) = joined_state.try_lock() {
                            *joined = true;
                        }
                        if let Ok(mut game_id) = game_id_lock.try_lock() {
                            *game_id = id;
                        }
                    });
                }
                ui.add_space(10.0);

                if let Some(error) = &self.error_message {
                    ui.colored_label(egui::Color32::RED, error);
                    ui.add_space(10.0);
                }

                if joined {
                    ui.label("🎮 Game in progress...");

                    let player = {
                        let player_guard = self.cached_player.try_lock().ok();
                        player_guard.and_then(|p| *p)
                    };

                    if let Some(player) = player {
                        self.render_board(ui, ctx_arc, player);
                    } else {
                        ui.label("🔄 Waiting for player assignment...");
                    }

                    self.display_game_status(ui);

                    if let Ok(game) = self.game_service.get_game().try_lock() {
                        if game.game_over {
                            ctx_arc.request_repaint();

                            if ui
                                .add_enabled(
                                    true,
                                    egui::Button::new(
                                        egui::RichText::new("🔄 Reset Game")
                                            .size(30.0)
                                            .color(egui::Color32::from_rgb(240, 148, 0)),
                                    ),
                                )
                                .clicked()
                            {
                                let game_service_clone = Arc::clone(&self.game_service);
                                tokio::spawn(async move {
                                    game_service_clone.reset_game().await;
                                });
                            }
                        }
                    }
                }
            });
        });
    }

    fn render_board(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, player: Player) {
        let game_arc = Arc::clone(&self.game_service.get_game());

        let game = match game_arc.try_lock() {
            Ok(game) => game,
            Err(_) => {
                error!("❌ Failed to acquire game lock in render_board()");
                return;
            }
        };

        let button_size = 100.0;

        ui.vertical_centered(|ui| {
            for row in 0..3 {
                ui.horizontal(|ui| {
                    ui.add_space(40.0);
                    for col in 0..3 {
                        let cell = game.board[row][col];

                        let can_move =
                            !game.game_over && player == game.current_turn && cell.is_none();

                        let button = ui.add_enabled(
                            can_move,
                            egui::Button::new(match cell {
                                Some(Player::X) => egui::RichText::new("X")
                                    .size(50.0)
                                    .color(egui::Color32::from_rgb(255, 99, 71)),
                                Some(Player::O) => egui::RichText::new("O")
                                    .size(50.0)
                                    .color(egui::Color32::from_rgb(34, 139, 34)),
                                None => egui::RichText::new(" ")
                                    .size(50.0)
                                    .color(egui::Color32::from_rgb(180, 180, 180)),
                            })
                            .min_size(egui::vec2(button_size, button_size)),
                        );

                        if button.clicked() && can_move {
                            let game_service_clone = Arc::clone(&self.game_service);
                            let ctx_clone = ctx.clone();

                            let game_id_clone = Arc::clone(&self.game_id);
                            tokio::spawn(async move {
                                let game_id = game_id_clone.lock().await.clone();
                                game_service_clone
                                    .make_move(game_id, player, row, col, ctx_clone.into())
                                    .await;
                            });
                        }
                    }
                });
            }
        });
    }

    fn display_game_status(&self, ui: &mut egui::Ui) {
        if let Ok(game) = self.game_service.get_game().try_lock() {
            let name_x = game
                .player_names
                .get(&Player::X)
                .cloned()
                .unwrap_or("X".to_string());
            let name_o = game
                .player_names
                .get(&Player::O)
                .cloned()
                .unwrap_or("O".to_string());

            let score_x = game.scores.get(&Player::X).cloned().unwrap_or(0);
            let score_o = game.scores.get(&Player::O).cloned().unwrap_or(0);

            let score_text = format!("{name_x} {} : {} {name_o}", score_x, score_o);

            ui.label(
                egui::RichText::new(score_text)
                    .size(24.0)
                    .color(egui::Color32::from_rgb(0, 191, 255)), // Light Blue
            );

            ui.add_space(10.0);

            if game.game_over {
                let status_message = if game.draw {
                    "It's a draw!".to_string()
                } else {
                    let winner_name = game
                        .player_names
                        .get(&game.current_turn)
                        .cloned()
                        .unwrap_or_else(|| format!("{:?}", game.current_turn));

                    format!("🏆 {} wins!", winner_name)
                };

                ui.label(
                    egui::RichText::new(status_message)
                        .size(30.0)
                        .color(egui::Color32::from_rgb(255, 0, 0)), // 🔴 red
                );
            } else {
                let current_turn_name = game
                    .player_names
                    .get(&game.current_turn)
                    .cloned()
                    .unwrap_or_else(|| format!("{:?}", game.current_turn));

                let turn_message = format!("🕐 {}'s turn", current_turn_name);

                ui.label(
                    egui::RichText::new(turn_message)
                        .size(30.0)
                        .color(egui::Color32::from_rgb(0, 255, 0)), // 🟢 green
                );
            }
        } else {
            ui.colored_label(egui::Color32::RED, "⚠️ Unable to fetch game state.");
        }
    }
}

#[tokio::main]
async fn main() {
    fmt::init();

    info!("🚀 Starting Tic-Tac-Toe Client...");

    if let Err(e) = eframe::run_native(
        "Tic-Tac-Toe",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(GameApp::default()))),
    ) {
        eprintln!("❌ Application crashed: {:?}", e);
    }
}
