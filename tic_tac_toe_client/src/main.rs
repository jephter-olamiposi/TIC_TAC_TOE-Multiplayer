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
        // Debug log to confirm the client's intent to join
        debug!(
            "Attempting to join game ID: {} with requested player: {:?}",
            game_id, player
        );

        // Send the join request to the server
        let response = self
            .client
            .post(format!("{}/join_game", self.server_url))
            .json(&serde_json::json!({ "game_id": game_id, "player": player }))
            .send();

        // Handle the response
        response
            .map_err(|e| {
                // Log network errors
                error!("Network error while trying to join game: {}", e);
                e.to_string()
            })
            .and_then(|resp| {
                if resp.status().is_success() {
                    // Parse the server's response to check the assigned player
                    resp.json::<Result<Player, String>>()
                        .map_err(|e| {
                            error!("Failed to parse join game response: {}", e);
                            e.to_string()
                        })
                        .and_then(|assigned_player| match assigned_player {
                            Ok(player) => {
                                info!(
                                    "Successfully joined game ID: {} as player {:?}",
                                    game_id, player
                                );
                                Ok(player)
                            }
                            Err(err) => {
                                // Log server-side rejection reasons
                                error!(
                                    "Join game request rejected by server for game ID {}: {}",
                                    game_id, err
                                );
                                Err(err)
                            }
                        })
                } else {
                    // Handle non-successful HTTP statuses
                    error!(
                        "Server responded with status {} when attempting to join game ID: {}",
                        resp.status(),
                        game_id
                    );
                    Err(format!(
                        "Failed to join game: Server returned error status {}",
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
        let websocket_url = self
            .server_url
            .replace("https://", "wss://")
            .replace("http://", "ws://")
            + "/ws";

        // Inside `start_websocket_listener` in GameService
        thread::spawn(move || {
            let mut retries = 0;
            let max_retries = 5;
            let backoff_duration = Duration::from_secs(2);

            loop {
                match connect(&websocket_url) {
                    Ok((mut socket, _)) => {
                        info!("WebSocket connected for game ID: {}", game_id);
                        retries = 0;

                        while let Ok(msg) = socket.read() {
                            if let Message::Text(text) = msg {
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
                    }
                    Err(err) => {
                        retries += 1;
                        error!(
                            "WebSocket connection failed: {}. Retry {}/{}",
                            err, retries, max_retries
                        );

                        if retries >= max_retries {
                            error!(
                                "WebSocket failed to reconnect after {} retries for game ID: {}",
                                max_retries, game_id
                            );
                            break;
                        }

                        thread::sleep(backoff_duration);
                    }
                }
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

                    // Create Game Section
                    if ui.button("Create Game").clicked() {
                        match self.game_service.create_game() {
                            Ok(game_id) => {
                                self.game_id = game_id;
                                self.input_game_id = self.game_id.clone();
                                info!("New game created with ID: {}", self.game_id);
                            }
                            Err(e) => {
                                self.error_message = Some(format!("Error creating game: {}", e));
                            }
                        }
                    }

                    // Join Game Section
                    ui.label("Game ID:");
                    ui.text_edit_singleline(&mut self.input_game_id);
                    if ui
                        .add_enabled(!self.loading, egui::Button::new("Join Game"))
                        .clicked()
                    {
                        if !self.input_game_id.is_empty() {
                            info!(
                                "Join game button clicked with game ID: {}",
                                self.input_game_id
                            );
                            self.join_game(ctx);
                        }
                    }

                    ui.add_space(20.0);

                    // Player Selection Section
                    if self.player.is_none() {
                        ui.label("Select Your Player:");
                        if ui.button("Play as X").clicked() {
                            match self.game_service.join_game(&self.game_id, Some(Player::X)) {
                                Ok(player) => {
                                    if player == Player::X {
                                        self.player = Some(Player::X);
                                        info!("Assigned Player X to game {}", self.game_id);
                                    } else {
                                        self.error_message =
                                            Some("Failed to assign Player X.".to_string());
                                    }
                                }
                                Err(e) => {
                                    self.error_message = Some(format!("Error: {}", e));
                                }
                            }
                        }

                        if ui.button("Play as O").clicked() {
                            match self.game_service.join_game(&self.game_id, Some(Player::O)) {
                                Ok(player) => {
                                    if player == Player::O {
                                        self.player = Some(Player::O);
                                        info!("Player O successfully assigned.");
                                    } else {
                                        self.error_message =
                                            Some("Failed to assign Player O.".to_string());
                                    }
                                }
                                Err(e) => {
                                    self.error_message = Some(format!("Error: {}", e));
                                }
                            }
                        }
                    }

                    ui.add_space(20.0);

                    // Error Message Section
                    if let Some(error) = &self.error_message {
                        ui.colored_label(egui::Color32::RED, error);
                        ui.add_space(10.0);
                    }

                    // Game Board and Status
                    if self.joined && self.player.is_some() {
                        self.render_board(ui);
                        ui.add_space(20.0);
                        self.display_game_status(ui);

                        if self.game_service.get_game().lock().unwrap().game_over {
                            if ui
                                .add_enabled(
                                    !self.loading,
                                    egui::Button::new(
                                        egui::RichText::new("Reset Game")
                                            .size(30.0)
                                            .color(egui::Color32::from_rgb(240, 148, 0)),
                                    ),
                                )
                                .clicked()
                            {
                                self.reset_game();
                            }
                        }
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
                    ui.add_space(40.0);
                    for col in 0..3 {
                        let cell = game.lock().unwrap().board[row][col];
                        let button = ui.add_enabled(
                            !game.lock().unwrap().game_over
                                && self.player.is_some()
                                && self.player.unwrap() == game.lock().unwrap().current_turn,
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

                        if button.clicked() && cell.is_none() {
                            self.make_move(self.player.unwrap(), row, col);
                        }
                    }
                });
            }
        });
    }

    fn display_game_status(&self, ui: &mut egui::Ui) {
        let game = self.game_service.get_game();
        let game = game.lock().unwrap();

        if game.game_over {
            let status_message = if game.draw {
                "It's a draw!".to_string()
            } else {
                format!("{:?} wins!", game.current_turn)
            };
            ui.label(
                egui::RichText::new(status_message)
                    .size(30.0)
                    .color(egui::Color32::from_rgb(255, 0, 0)),
            );
        } else {
            let turn_message = format!("{:?}'s turn", game.current_turn);
            ui.label(
                egui::RichText::new(turn_message)
                    .size(30.0)
                    .color(egui::Color32::from_rgb(0, 255, 0)),
            );
        }
    }

    fn join_game(&mut self, ctx: &egui::Context) {
        self.loading = true;
        self.error_message = None;
        self.game_id = self.input_game_id.clone();

        info!("Attempting to join game with ID: {}", self.game_id);

        match self.game_service.join_game(&self.game_id, None) {
            Ok(player) => {
                self.player = Some(player);
                self.joined = true;
                self.game_service.fetch_game_state(&self.game_id).unwrap();

                // Wrap the context in an Arc before passing it
                self.game_service
                    .start_websocket_listener(self.game_id.clone(), Arc::new(ctx.clone()));
                info!("Successfully joined game with ID: {}", self.game_id);
            }
            Err(e) => {
                self.error_message = Some(format!("Failed to join game: {}", e));
            }
        }

        self.loading = false;
    }

    fn make_move(&mut self, player: Player, row: usize, col: usize) {
        if self.loading {
            return;
        }

        info!("Player {:?} making move at ({}, {})", player, row, col);

        if let Err(e) = self.game_service.make_move(&self.game_id, player, row, col) {
            self.error_message = Some(format!("Error making move: {}", e));
        }
    }

    fn reset_game(&mut self) {
        self.loading = true;
        self.error_message = None;

        info!("Resetting game with ID: {}", self.game_id);

        if let Err(e) = self.game_service.reset_game(&self.game_id) {
            self.error_message = Some(format!("Error resetting game: {}", e));
        }

        self.loading = false;
    }
}

fn main() -> Result<(), eframe::Error> {
    eframe::run_native(
        "Tic-Tac-Toe (Multiplayer)",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(GameApp::default()))),
    )
}
