use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::thread;
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

fn main() -> Result<(), eframe::Error> {
    println!("Starting Tic-Tac-Toe client...");
    eframe::run_native(
        "Tic-Tac-Toe (Online)",
        eframe::NativeOptions::default(),
        Box::new(|_cc| {
            // Create and return the game app instance
            Ok(Box::new(GameApp::default()))
        }),
    )
}

struct GameApp {
    client: Client,
    server_url: String,
    game: Arc<Mutex<Game>>,
    game_id: String,
    input_game_id: String,
}

impl Default for GameApp {
    fn default() -> Self {
        println!("Initializing GameApp...");
        let game = Arc::new(Mutex::new(Game {
            board: [[None; 3]; 3],
            current_turn: Player::X,
            game_over: false,
            draw: false,
        }));

        Self {
            client: Client::new(),
            server_url: "127.0.0.1:3000".to_string(),
            game,
            game_id: String::new(),
            input_game_id: String::new(),
        }
    }
}

impl GameApp {
    // Render the game board as a 3x3 grid
    fn render_board(&mut self, ui: &mut egui::Ui) {
        let button_size = ui.available_width() / 4.0; // Dynamically adjust button size

        // Vertical layout for rows
        ui.vertical(|ui| {
            for row in 0..3 {
                ui.horizontal(|ui| {
                    for col in 0..3 {
                        let cell = &self.game.lock().unwrap().board[row][col];
                        let button = ui.add_enabled(
                            !self.game.lock().unwrap().game_over, // Disable buttons if the game is over
                            egui::Button::new(match cell {
                                Some(Player::X) => egui::RichText::new("X")
                                    .size(80.0)
                                    .color(egui::Color32::from_rgb(255, 99, 71)), // Red
                                Some(Player::O) => egui::RichText::new("O")
                                    .size(80.0)
                                    .color(egui::Color32::from_rgb(34, 139, 34)), // Green
                                None => egui::RichText::new(" ")
                                    .size(80.0)
                                    .color(egui::Color32::from_rgb(180, 180, 180)), // Gray
                            })
                            .min_size(egui::vec2(button_size, button_size)),
                        );

                        if button.clicked() && cell.is_none() {
                            println!("Button clicked at ({}, {})", row, col); // Debugging
                            let response = self
                                .client
                                .post(format!("http://{}/make_move", self.server_url))
                                .json(&serde_json::json!({ "game_id": self.game_id, "x": row, "y": col }))
                                .send();

                            if response.is_ok() {
                                // Fetch the updated game state from the server
                                if let Ok(g) = self
                                    .client
                                    .post(format!("http://{}/state", self.server_url))
                                    .json(&self.game_id)
                                    .send()
                                    .and_then(|resp| resp.json::<Game>())
                                {
                                    let mut game = self.game.lock().unwrap();
                                    *game = g;
                                }
                            }
                        }
                    }
                });
            }
        });
    }

    fn is_full(&self) -> bool {
        self.client
            .post(format!("http://{}/is_full", self.server_url))
            .json(&self.game_id)
            .send()
            .ok()
            .map(|resp| resp.json::<bool>().unwrap_or(false))
            .unwrap_or(false)
    }

    fn check_winner(&self) -> Option<Player> {
        self.client
            .post(format!("http://{}/check_winner", self.server_url))
            .json(&self.game_id)
            .send()
            .ok()
            .and_then(|resp| resp.json::<Option<Player>>().ok())
            .flatten()
    }

    // Reset the game state
    fn reset_game(&mut self) {
        let _ = self
            .client
            .post(format!("http://{}/reset", self.server_url))
            .json(&self.game_id)
            .send();

        // Fetch the updated game state from the server
        if let Ok(g) = self
            .client
            .post(format!("http://{}/state", self.server_url))
            .json(&self.game_id)
            .send()
            .and_then(|resp| resp.json::<Game>())
        {
            let mut game = self.game.lock().unwrap();
            *game = g;
        }
    }

    // Join or create a game session
    fn join_game(&mut self) {
        self.game_id = self.input_game_id.clone();
        // Fetch the initial game state from the server
        if let Ok(g) = self
            .client
            .post(format!("http://{}/state", self.server_url))
            .json(&self.game_id)
            .send()
            .and_then(|resp| resp.json::<Game>())
        {
            let mut game = self.game.lock().unwrap();
            *game = g;
        }

        // Connect to the WebSocket for real-time updates
        let server_url_clone = self.server_url.clone();
        let game_id_clone = self.game_id.clone();
        let game_clone = Arc::clone(&self.game);

        thread::spawn(move || {
            println!("Connecting to WebSocket...");
            let (mut socket, _) =
                connect(&format!("ws://{}/ws", server_url_clone)).expect("Can't connect");
            loop {
                let msg = socket.read().expect("Error reading message");
                if let Message::Text(text) = msg {
                    let (received_game_id, received_game): (String, Game) =
                        serde_json::from_str(&text).unwrap();
                    if received_game_id == game_id_clone {
                        let mut game = game_clone.lock().unwrap();
                        *game = received_game;
                    }
                }
            }
        });
    }
}

impl eframe::App for GameApp {
    // Main update loop for rendering the UI
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        println!("Updating UI..."); // Debugging
                                    // 1) Fetch the current state from the server
        if !self.game_id.is_empty() {
            if let Ok(g) = self
                .client
                .post(format!("http://{}/state", self.server_url))
                .json(&self.game_id)
                .send()
                .and_then(|resp| resp.json::<Game>())
            {
                let mut game = self.game.lock().unwrap();
                *game = g;
            }
        }

        // Set custom background color for the window
        ctx.style_mut(|style| {
            style.visuals.window_fill = egui::Color32::from_rgb(30, 30, 30); // Dark gray
        });

        // Render the central UI panel
        egui::CentralPanel::default().show(ctx, |ui| {
            // Center the game grid and related controls
            ui.vertical_centered(|ui| {
                // Input field for game ID
                ui.horizontal(|ui| {
                    ui.label("Game ID:");
                    ui.text_edit_singleline(&mut self.input_game_id);
                    if ui.button("Join Game").clicked() {
                        self.join_game();
                    }
                });

                ui.add_space(20.0); // Add vertical spacing after the input field

                if !self.game_id.is_empty() {
                    // Dynamically calculate padding for horizontal centering
                    ui.horizontal(|ui| {
                        let available_space = ui.available_width();
                        let padding = (available_space - 360.0) / 2.0;
                        ui.add_space(padding); // Left padding
                        ui.centered_and_justified(|ui| {
                            println!("Rendering board..."); // Debugging
                            self.render_board(ui)
                        }); // Render the game board
                        ui.add_space(padding); // Right padding
                    });

                    ui.add_space(20.0); // Add vertical spacing after the grid

                    // Check for winner or draw
                    if let Some(winner) = self.check_winner() {
                        self.game.lock().unwrap().game_over = true;
                        ui.label(
                            egui::RichText::new(format!("{:?} wins!", winner))
                                .size(50.0)
                                .color(egui::Color32::from_rgb(255, 223, 0)), // Yellow
                        );

                        // Reset button
                        if ui
                            .button(
                                egui::RichText::new("Reset Game")
                                    .size(50.0)
                                    .color(egui::Color32::from_rgb(240, 148, 0)), // Orange
                            )
                            .clicked()
                        {
                            println!("Reset button clicked"); // Debugging
                            self.reset_game();
                        }
                    } else if self.is_full() {
                        self.game.lock().unwrap().game_over = true;
                        self.game.lock().unwrap().draw = true;
                        ui.label(
                            egui::RichText::new("It's a draw!")
                                .size(50.0)
                                .color(egui::Color32::from_rgb(200, 200, 200)), // Gray
                        );

                        // Reset button
                        if ui
                            .button(
                                egui::RichText::new("Reset Game")
                                    .size(50.0)
                                    .color(egui::Color32::from_rgb(240, 148, 0)),
                            )
                            .clicked()
                        {
                            println!("Reset button clicked"); // Debugging
                            self.reset_game();
                        }
                    } else {
                        // Display the current player's turn
                        ui.label(
                            egui::RichText::new(format!(
                                "{:?}'s turn",
                                self.game.lock().unwrap().current_turn
                            ))
                            .size(50.0)
                            .color(egui::Color32::from_rgb(160, 160, 255)), // Light blue
                        );
                    }
                }
            });
        });
    }
}
