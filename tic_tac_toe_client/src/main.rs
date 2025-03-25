use tracing::info;
use tracing_subscriber::fmt;

mod game_app;
mod game_service;

use game_app::GameApp;

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
