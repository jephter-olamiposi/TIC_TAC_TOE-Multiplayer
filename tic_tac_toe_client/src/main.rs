use eframe::egui;
mod game; // Import the game module
use game::{Game, Player};

fn main() -> Result<(), eframe::Error> {
    // Run the application with default native options
    eframe::run_native(
        "Tic-Tac-Toe", // Title of the window
        eframe::NativeOptions {
            ..Default::default() // Default window size
        },
        Box::new(|_cc| {
            // Create and return the game app instance
            Ok(Box::new(GameApp::default()))
        }),
    )
}

#[derive(Default)]
struct GameApp {
    game: Game,      // Holds the game state (board, current turn)
    game_over: bool, // Tracks if the game is over (win or draw)
    draw: bool,      // Tracks if the game ended in a draw
}

impl eframe::App for GameApp {
    // Main update loop for rendering the UI
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Set custom background color for the window
        ctx.style_mut(|style| {
            style.visuals.window_fill = egui::Color32::from_rgb(30, 30, 30); // Dark gray
        });

        // Render the central UI panel
        egui::CentralPanel::default().show(ctx, |ui| {
            // Center the game grid and related controls
            ui.vertical_centered(|ui| {
                // Dynamically calculate padding for horizontal centering
                ui.horizontal(|ui| {
                    let available_space = ui.available_width();
                    let padding = (available_space - 360.0) / 2.0;
                    ui.add_space(padding); // Left padding
                    ui.centered_and_justified(|ui| self.render_board(ui)); // Render the game board
                    ui.add_space(padding); // Right padding
                });

                ui.add_space(20.0); // Add vertical spacing after the grid

                // Check for winner or draw
                if let Some(winner) = self.game.check_winner() {
                    self.game_over = true;
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
                        self.reset_game();
                    }
                } else if self.game.is_full() {
                    self.game_over = true;
                    self.draw = true;
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
                        self.reset_game();
                    }
                } else {
                    // Display the current player's turn
                    ui.label(
                        egui::RichText::new(format!("{:?}'s turn", self.game.current_turn))
                            .size(50.0)
                            .color(egui::Color32::from_rgb(160, 160, 255)), // Light blue
                    );
                }
            });
        });
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
                        let cell = &self.game.board[row][col];
                        let button = ui.add_enabled(
                            !self.game_over, // Disable buttons if the game is over
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
                            let _ = self.game.make_move(row, col);
                        }
                    }
                });
            }
        });

        // Perform AI move if it's O's turn
        // if self.game.current_turn == Player::O && !self.game_over {
        //     if let Err(e) = self.game.ai_move() {
        //         ui.label(egui::RichText::new(format!("AI Error: {}", e)).color(egui::Color32::RED));
        //     }
        // }
    }

    // Reset the game state
    fn reset_game(&mut self) {
        *self = Self::default();
    }
}
