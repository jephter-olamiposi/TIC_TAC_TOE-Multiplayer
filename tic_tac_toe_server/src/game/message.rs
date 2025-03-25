use serde::{Deserialize, Serialize};

use super::models::Player;

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinGameRequest {
    pub game_id: String,
    pub player: Option<Player>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MoveRequest {
    pub game_id: String,
    pub player: Player,
    pub x: usize,
    pub y: usize,
}
