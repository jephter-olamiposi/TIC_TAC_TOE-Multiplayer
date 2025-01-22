use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info};
use tungstenite::{connect, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Player {
    X,
    O,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub board: [[Option<Player>; 3]; 3],
    pub current_turn: Player,
    pub game_over: bool,
    pub draw: bool,
    pub players: Vec<Player>,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            board: [[None; 3]; 3],
            current_turn: Player::X,
            game_over: false,
            draw: false,
            players: Vec::new(),
        }
    }
}

pub struct GameService {
    client: Client,
    server_url: String,
    game: Arc<Mutex<Game>>,
}

impl GameService {
    pub fn new(server_url: String) -> Self {
        Self {
            client: Client::new(),
            server_url,
            game: Arc::new(Mutex::new(Game::default())),
        }
    }

    pub fn get_game(&self) -> Arc<Mutex<Game>> {
        Arc::clone(&self.game)
    }

    pub fn create_game(&self) -> Result<String, String> {
        let response = self
            .client
            .post(format!("{}/create_game", self.server_url))
            .send()
            .map_err(|e| e.to_string())?;

        response.json::<String>().map_err(|e| e.to_string())
    }

    pub fn join_game(&self, game_id: &str, player: Option<Player>) -> Result<Player, String> {
        debug!(
            "Attempting to join game ID: {} with player: {:?}",
            game_id, player
        );

        let response = self
            .client
            .post(format!("{}/join_game", self.server_url))
            .json(&serde_json::json!({ "game_id": game_id, "player": player }))
            .send();

        response
            .map_err(|e| {
                error!("Network error while joining game: {}", e);
                e.to_string()
            })
            .and_then(|resp| {
                if resp.status().is_success() {
                    let assigned_player = resp.json::<Result<Player, String>>().map_err(|e| {
                        error!("Error parsing join game response: {}", e);
                        e.to_string()
                    })?;

                    match assigned_player {
                        Ok(player) => {
                            info!(
                                "Successfully joined game ID: {} as player {:?}",
                                game_id, player
                            );
                            Ok(player)
                        }
                        Err(err) => {
                            error!("Server rejected join game request: {}", err);
                            Err(err)
                        }
                    }
                } else {
                    error!(
                        "Failed to join game ID: {}. Server responded with status: {}",
                        game_id,
                        resp.status()
                    );
                    Err(format!(
                        "Failed to join game: Server error (status {})",
                        resp.status()
                    ))
                }
            })
    }

    pub fn fetch_game_state(&self, game_id: &str) -> Result<(), String> {
        let response = self
            .client
            .post(format!("{}/state", self.server_url))
            .json(&game_id)
            .send();

        response.map_err(|e| e.to_string()).and_then(|resp| {
            if resp.status().is_success() {
                let new_game = resp.json::<Game>().map_err(|e| e.to_string())?;
                let mut game = self.game.lock().unwrap();
                *game = new_game;
                info!("Fetched new game state for game ID: {}", game_id);
                Ok(())
            } else if resp.status() == 404 {
                Err("Game not found. It may have expired.".to_string())
            } else {
                Err(format!("Server error: {}", resp.status()))
            }
        })
    }

    pub fn make_move(
        &self,
        game_id: &str,
        player: Player,
        row: usize,
        col: usize,
    ) -> Result<(), String> {
        let response = self
            .client
            .post(format!("{}/make_move", self.server_url))
            .json(&serde_json::json!({
                "game_id": game_id,
                "player": player,
                "x": row,
                "y": col
            }))
            .send();

        response.map_err(|e| e.to_string()).and_then(|resp| {
            if resp.status().is_success() {
                self.fetch_game_state(game_id).map(|_| ())
            } else {
                Err(format!("Server error: {}", resp.status()))
            }
        })
    }

    pub fn reset_game(&self, game_id: &str) -> Result<(), String> {
        let response = self
            .client
            .post(format!("{}/reset", self.server_url))
            .json(&game_id)
            .send();

        response.map_err(|e| e.to_string()).and_then(|resp| {
            if resp.status().is_success() {
                let mut game = self.game.lock().unwrap();
                *game = Game::default();
                info!("Game with ID: {} has been reset", game_id);
                Ok(())
            } else {
                Err(format!("Server error: {}", resp.status()))
            }
        })
    }

    pub fn start_websocket_listener(&self, game_id: String, ctx: Arc<egui::Context>) {
        let game_clone = self.get_game();
        let server_url = "wss://tic-tac-toe-multiplayer-zg0e.onrender.com/ws".to_string();

        thread::spawn(move || {
            let mut retries = 0;
            let max_retries = 5;
            let backoff_duration = Duration::from_secs(2);

            while retries < max_retries {
                if let Ok((mut socket, _)) = connect(&server_url) {
                    info!("WebSocket connection established for game ID: {}", game_id);
                    retries = 0;

                    while let Ok(msg) = socket.read() {
                        if let Message::Text(text) = msg {
                            debug!("WebSocket message received: {}", text);
                            if let Ok((received_game_id, received_game)) =
                                serde_json::from_str::<(String, Game)>(&text)
                            {
                                if received_game_id == game_id {
                                    let mut game = game_clone.lock().unwrap();
                                    *game = received_game;
                                    ctx.request_repaint();
                                    info!("Game state updated for game ID: {}", game_id);
                                }
                            }
                        }
                    }
                } else {
                    retries += 1;
                    error!(
                        "WebSocket connection failed for game ID: {}. Retry {}/{}",
                        game_id, retries, max_retries
                    );
                    thread::sleep(backoff_duration * retries as u32);
                }
            }

            if retries >= max_retries {
                error!(
                    "WebSocket connection failed after {} retries for game ID: {}",
                    max_retries, game_id
                );
            }
        });
    }
}

pub struct GameApp {
    game_service: GameService,
    game_id: String,
    input_game_id: String,
    joined: bool,
    loading: bool,
    error_message: Option<String>,
    player: Option<Player>,
}

impl Default for GameApp {
    fn default() -> Self {
        Self {
            game_service: GameService::new(
                "https://tic-tac-toe-multiplayer-zg0e.onrender.com".to_string(),
            ),
            game_id: String::new(),
            input_game_id: String::new(),
            joined: false,
            loading: false,
            error_message: None,
            player: None,
        }
    }
}

impl eframe::App for GameApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.group(|ui| {
                    ui.set_width(400.0);
                    ui.set_height(500.0);

                    if ui.button("Create Game").clicked() {
                        match self.game_service.create_game() {
                            Ok(game_id) => {
                                self.game_id = game_id;
                                self.input_game_id = self.game_id.clone();
                                info!("Game created with ID: {}", self.game_id);
                            }
                            Err(e) => {
                                self.error_message = Some(format!("Error creating game: {}", e));
                            }
                        }
                    }

                    ui.label("Game ID:");
                    ui.text_edit_singleline(&mut self.input_game_id);
                    if ui
                        .add_enabled(!self.loading, egui::Button::new("Join Game"))
                        .clicked()
                    {
                        self.join_game(ctx);
                    }

                    if let Some(error) = &self.error_message {
                        ui.colored_label(egui::Color32::RED, error);
                    }

                    if self.joined && self.player.is_some() {
                        self.render_board(ui);
                    }
                });
            });
        });
    }
}

impl GameApp {
    fn render_board(&mut self, ui: &mut egui::Ui) {
        let game = self.game_service.get_game();
        let button_size = 100.0;

        ui.vertical(|ui| {
            for row in 0..3 {
                ui.horizontal(|ui| {
                    for col in 0..3 {
                        let cell = game.lock().unwrap().board[row][col];
                        let button = ui.add_enabled(
                            !game.lock().unwrap().game_over,
                            egui::Button::new(match cell {
                                Some(Player::X) => "X",
                                Some(Player::O) => "O",
                                None => " ",
                            })
                            .min_size(egui::vec2(button_size, button_size)),
                        );

                        if button.clicked() && cell.is_none() {
                            self.make_move(row, col);
                        }
                    }
                });
            }
        });
    }

    fn join_game(&mut self, ctx: &egui::Context) {
        if let Ok(player) = self.game_service.join_game(&self.input_game_id, None) {
            self.player = Some(player);
            self.joined = true;
            self.game_service
                .start_websocket_listener(self.input_game_id.clone(), Arc::new(ctx.clone()));
        }
    }

    fn make_move(&mut self, row: usize, col: usize) {
        if let Some(player) = self.player {
            if let Err(e) = self.game_service.make_move(&self.game_id, player, row, col) {
                self.error_message = Some(format!("Error making move: {}", e));
            }
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    eframe::run_native(
        "Tic-Tac-Toe Multiplayer",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(GameApp::default()))),
    )
}
