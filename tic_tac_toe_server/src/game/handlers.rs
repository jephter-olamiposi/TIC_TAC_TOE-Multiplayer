use anyhow::Result;
use serde_json::json;

use crate::app_state::AppState;
use crate::game::{models::Game, models::Player};

use std::sync::Arc;
use tracing::{error, info};

pub async fn handle_join_game(
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

pub async fn handle_make_move(
    parsed: &serde_json::Value,
    state: &Arc<AppState>,
    socket: &mut axum::extract::ws::WebSocket,
) -> Result<()> {
    let game_id = parsed["game_id"].as_str().unwrap_or("").to_string();
    let x = parsed["x"].as_u64().unwrap_or(100) as usize;
    let y = parsed["y"].as_u64().unwrap_or(100) as usize;

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

pub async fn handle_reset_game(parsed: &serde_json::Value, state: &Arc<AppState>) -> Result<()> {
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
