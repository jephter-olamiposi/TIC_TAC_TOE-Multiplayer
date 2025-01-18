use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::{thread, time::Duration};
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
}

impl Default for Game {
    fn default() -> Self {
        Game {
            board: [[None; 3]; 3],
            current_turn: Player::X,
            game_over: false,
            draw: false,
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

    pub fn fetch_game_state(&self, game_id: &str) -> Result<(), String> {
        let response = self
            .client
            .post(format!("{}/state", self.server_url))
            .json(&game_id)
            .send();

        response.map_err(|e| e.to_string()).and_then(|resp| {
            let new_game = resp.json::<Game>().map_err(|e| e.to_string())?;
            let mut game = self.game.lock().unwrap();
            *game = new_game;
            Ok(())
        })
    }

    pub fn make_move(&self, game_id: &str, row: usize, col: usize) -> Result<(), String> {
        let response = self
            .client
            .post(format!("{}/make_move", self.server_url))
            .json(&serde_json::json!({ "game_id": game_id, "x": row, "y": col }))
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
                Ok(())
            } else {
                Err(format!("Server error: {}", resp.status()))
            }
        })
    }
}

pub struct GameApp {
    game_service: GameService,
    game_id: String,
    input_game_id: String,
    joined: bool,
    loading: bool,
    error_message: Option<String>,
}

impl Default for GameApp {
    fn default() -> Self {
        Self {
            game_service: GameService::new(
                "https://tic-tac-toe-multiplayer-jzxq.onrender.com".to_string(),
            ),
            game_id: String::new(),
            input_game_id: String::new(),
            joined: false,
            loading: false,
            error_message: None,
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

                    // Join Game Section
                    ui.vertical_centered(|ui| {
                        ui.label("Game ID:");
                        ui.text_edit_singleline(&mut self.input_game_id);
                        if ui
                            .add_enabled(!self.loading, egui::Button::new("Join Game"))
                            .clicked()
                        {
                            if !self.input_game_id.is_empty() {
                                self.join_game();
                            }
                        }
                    });

                    ui.add_space(20.0);

                    // Error Message Section
                    if let Some(error) = &self.error_message {
                        ui.colored_label(egui::Color32::RED, error);
                        ui.add_space(10.0);
                    }

                    // Game Board and Status
                    if self.joined {
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
                                            .color(egui::Color32::from_rgb(240, 148, 0)), // Orange
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
                            !game.lock().unwrap().game_over,
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
                            self.make_move(row, col);
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

    fn join_game(&mut self) {
        self.loading = true;
        self.error_message = None;
        self.game_id = self.input_game_id.clone();

        if self.game_service.fetch_game_state(&self.game_id).is_ok() {
            self.joined = true;

            // WebSocket Connection for Real-Time Updates with Retries
            let game_clone = self.game_service.get_game();
            let server_url = format!("ws://127.0.0.1:3000/ws");
            let game_id = self.game_id.clone();

            thread::spawn(move || {
                let mut retries = 0;

                loop {
                    if let Ok((mut socket, _)) = connect(server_url.clone()) {
                        while let Ok(msg) = socket.read() {
                            if let Message::Text(text) = msg {
                                if let Ok((received_game_id, received_game)) =
                                    serde_json::from_str::<(String, Game)>(&text)
                                {
                                    if received_game_id == game_id {
                                        let mut game = game_clone.lock().unwrap();
                                        *game = received_game;
                                    }
                                }
                            }
                        }
                    } else {
                        retries += 1;
                        if retries > 5 {
                            eprintln!("WebSocket connection failed after retries.");
                            break;
                        }
                        thread::sleep(Duration::from_secs(2 * retries)); // Exponential backoff
                    }
                }
            });
        } else {
            self.error_message = Some("Failed to join game. Please check the Game ID.".to_string());
        }
        self.loading = false;
    }

    fn make_move(&mut self, row: usize, col: usize) {
        if self.loading {
            return;
        }

        if let Err(e) = self.game_service.make_move(&self.game_id, row, col) {
            self.error_message = Some(format!("Error making move: {}", e));
        }
    }

    fn reset_game(&mut self) {
        self.loading = true;
        self.error_message = None;

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
