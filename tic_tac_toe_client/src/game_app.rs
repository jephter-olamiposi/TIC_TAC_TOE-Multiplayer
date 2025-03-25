use crate::game_service::{GameService, Player};

use eframe::egui;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::error;

#[derive(Clone)]
pub struct GameApp {
    game_service: Arc<GameService>,
    game_id: Arc<Mutex<String>>,
    input_game_id: String,
    input_player_name: String,
    joined: Arc<Mutex<bool>>,
    error_message: Option<String>,
    cached_player: Arc<Mutex<Option<Player>>>,
}
impl Default for GameApp {
    fn default() -> Self {
        Self {
            game_service: Arc::new(GameService::new(
                "https://tic-tac-toe-multiplayer-zg0e.onrender.com".to_string(),
            )),
            game_id: Arc::new(Mutex::new(String::new())),
            input_game_id: String::new(),
            input_player_name: String::new(),
            joined: Arc::new(Mutex::new(false)),
            error_message: None,
            cached_player: Arc::new(Mutex::new(None)),
        }
    }
}

impl eframe::App for GameApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let game_service = Arc::clone(&self.game_service);
        let cached_player = Arc::clone(&self.cached_player);
        let joined_state = Arc::clone(&self.joined);

        tokio::spawn(async move {
            if let Some(player) = game_service.get_player().await {
                if let Ok(mut cached) = cached_player.try_lock() {
                    *cached = Some(player);
                }
            }
        });

        let joined = joined_state.try_lock().map(|guard| *guard).unwrap_or(false);

        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        egui::CentralPanel::default().show(ctx, |ui| {
            self.handle_game_ui(ui, &Arc::new(ctx.clone()), joined);
        });
    }
}

impl GameApp {
    fn handle_game_ui(&mut self, ui: &mut egui::Ui, ctx_arc: &Arc<egui::Context>, joined: bool) {
        ui.vertical_centered(|ui| {
            ui.group(|ui| {
                ui.set_width(400.0);
                ui.set_height(500.0);

                if !joined {
                    ui.label("Your Name:");

                    ui.add_space(3.0);

                    ui.text_edit_singleline(&mut self.input_player_name);
                }

                ui.add_space(10.0);

                ui.label("Game ID:");

                ui.add_space(3.0);

                ui.text_edit_singleline(&mut self.input_game_id);

                ui.add_space(10.0);

                let can_join = !self.input_game_id.trim().is_empty()
                    && !self.input_player_name.trim().is_empty();
                if ui
                    .add_enabled(
                        can_join,
                        egui::Button::new("Join Game").min_size(egui::vec2(100.0, 30.0)),
                    )
                    .clicked()
                {
                    let ctx_clone = Arc::clone(ctx_arc);
                    let game_service_clone = Arc::clone(&self.game_service);
                    let input_game_id = self.input_game_id.clone();
                    let player_name = self.input_player_name.clone();
                    let joined_state = Arc::clone(&self.joined);
                    let game_id_lock = Arc::clone(&self.game_id);

                    tokio::spawn(async move {
                        let id = input_game_id.clone();
                        game_service_clone
                            .join_game(input_game_id, player_name, ctx_clone)
                            .await;

                        if let Ok(mut joined) = joined_state.try_lock() {
                            *joined = true;
                        }
                        if let Ok(mut game_id) = game_id_lock.try_lock() {
                            *game_id = id;
                        }
                    });
                }
                ui.add_space(10.0);

                if let Some(error) = &self.error_message {
                    ui.colored_label(egui::Color32::RED, error);
                    ui.add_space(10.0);
                }

                if joined {
                    ui.label("🎮 Game in progress...");

                    let player = {
                        let player_guard = self.cached_player.try_lock().ok();
                        player_guard.and_then(|p| *p)
                    };

                    if let Some(player) = player {
                        self.render_board(ui, ctx_arc, player);
                    } else {
                        ui.label("🔄 Waiting for player assignment...");
                    }

                    self.display_game_status(ui);

                    ui.add_space(5.0);

                    if let Ok(game) = self.game_service.get_game().try_lock() {
                        if game.game_over {
                            ctx_arc.request_repaint();

                            if ui
                                .add_enabled(
                                    true,
                                    egui::Button::new(
                                        egui::RichText::new("🔄 Reset Game")
                                            .size(25.0)
                                            .color(egui::Color32::from_rgb(240, 148, 0)),
                                    ),
                                )
                                .clicked()
                            {
                                let game_service_clone = Arc::clone(&self.game_service);
                                tokio::spawn(async move {
                                    game_service_clone.reset_game().await;
                                });
                            }
                        }
                    }
                }
            });
        });
    }

    fn render_board(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, player: Player) {
        let game_arc = Arc::clone(&self.game_service.get_game());

        let game = match game_arc.try_lock() {
            Ok(game) => game,
            Err(_) => {
                error!("❌ Failed to acquire game lock in render_board()");
                return;
            }
        };

        let button_size = 100.0;

        ui.vertical_centered(|ui| {
            for row in 0..3 {
                ui.horizontal(|ui| {
                    ui.add_space(40.0);
                    for col in 0..3 {
                        let cell = game.board[row][col];

                        let can_move =
                            !game.game_over && player == game.current_turn && cell.is_none();

                        let button = ui.add_enabled(
                            can_move,
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

                        if button.clicked() && can_move {
                            let game_service_clone = Arc::clone(&self.game_service);
                            let ctx_clone = ctx.clone();

                            let game_id_clone = Arc::clone(&self.game_id);
                            tokio::spawn(async move {
                                let game_id = game_id_clone.lock().await.clone();
                                game_service_clone
                                    .make_move(game_id, player, row, col, ctx_clone.into())
                                    .await;
                            });
                        }
                    }
                });
            }
        });
    }

    fn display_game_status(&self, ui: &mut egui::Ui) {
        if let Ok(game) = self.game_service.get_game().try_lock() {
            let name_x = game
                .player_names
                .get(&Player::X)
                .cloned()
                .unwrap_or("X".to_string());
            let name_o = game
                .player_names
                .get(&Player::O)
                .cloned()
                .unwrap_or("O".to_string());

            let score_x = game.scores.get(&Player::X).cloned().unwrap_or(0);
            let score_o = game.scores.get(&Player::O).cloned().unwrap_or(0);

            let score_text = format!("{name_x} {} : {} {name_o}", score_x, score_o);

            ui.label(
                egui::RichText::new(score_text)
                    .size(24.0)
                    .color(egui::Color32::from_rgb(0, 191, 255)),
            );

            ui.add_space(10.0);

            if game.game_over {
                let status_message = if game.draw {
                    "It's a draw!".to_string()
                } else {
                    let winner_name = game
                        .player_names
                        .get(&game.current_turn)
                        .cloned()
                        .unwrap_or_else(|| format!("{:?}", game.current_turn));

                    format!("🏆 {} wins!", winner_name)
                };

                ui.label(
                    egui::RichText::new(status_message)
                        .size(30.0)
                        .color(egui::Color32::from_rgb(255, 0, 0)),
                );
            } else {
                let current_turn_name = game
                    .player_names
                    .get(&game.current_turn)
                    .cloned()
                    .unwrap_or_else(|| format!("{:?}", game.current_turn));

                let turn_message = format!("🕐 {}'s turn", current_turn_name);

                ui.label(
                    egui::RichText::new(turn_message)
                        .size(30.0)
                        .color(egui::Color32::from_rgb(0, 255, 0)),
                );
            }
        } else {
            ui.colored_label(egui::Color32::RED, "⚠️ Unable to fetch game state.");
        }
    }
}
