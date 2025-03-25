use crate::app_state::AppState;

use std::{sync::Arc, time::Duration};
use tracing::info;

pub async fn cleanup_inactive_games(app_state: Arc<AppState>) {
    let timeout = Duration::from_secs(1200); // 20 minutes

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
