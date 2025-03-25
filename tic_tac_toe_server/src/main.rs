use axum::{routing::get, Router};
use std::env;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

mod app_state;
mod cleanup;
mod game;
mod ws_socket;

use app_state::AppState;
use cleanup::cleanup_inactive_games;
use ws_socket::ws_handler;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .init();

    let (tx, _) = broadcast::channel(500);
    let app_state = Arc::new(AppState::new(tx));

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
