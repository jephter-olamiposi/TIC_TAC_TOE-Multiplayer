use crate::game_service::model::{Game, Player};
use eframe::egui;
use futures_util::stream::StreamExt;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::SinkExt;
use std::time::Instant;
use std::{sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tracing::{error, info};
use tungstenite::Message;

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

        if !self.is_connected().await {
            error!("🔌 WebSocket is disconnected. Trying to reconnect...");

            if let Err(e) = self.reconnect(game_id.clone(), ctx.clone()).await {
                error!("❌ Reconnection failed: {}", e);
                return;
            }

            tokio::time::sleep(Duration::from_millis(150)).await;
        }

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
