use crate::app_state::AppState;
use crate::game::handlers::{handle_join_game, handle_make_move, handle_reset_game};

use anyhow::Result;
use axum::extract::{State, WebSocketUpgrade};
use serde_json::json;
use std::sync::Arc;
use tracing::{error, info};

#[axum::debug_handler]
pub async fn ws_handler(
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
