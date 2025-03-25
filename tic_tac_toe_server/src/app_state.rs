use crate::game::models::Game;

use std::{collections::HashMap, sync::Arc};
use tokio::sync::broadcast;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub games: Arc<RwLock<HashMap<String, Game>>>,
    pub tx: broadcast::Sender<(String, Game)>,
}
impl AppState {
    pub fn new(tx: broadcast::Sender<(String, Game)>) -> Self {
        AppState {
            games: Arc::new(RwLock::new(HashMap::new())),
            tx,
        }
    }
}
