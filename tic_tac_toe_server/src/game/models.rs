use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::SystemTime};
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
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
    pub last_activity: SystemTime,
    pub players: Vec<Player>,
    pub scores: HashMap<Player, u32>,
    pub player_names: HashMap<Player, String>,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            board: [[None; 3]; 3],
            current_turn: Player::X,
            game_over: false,
            draw: false,
            last_activity: SystemTime::now(),
            players: Vec::new(),
            player_names: HashMap::new(),
            scores: [(Player::X, 0), (Player::O, 0)].into_iter().collect(),
        }
    }
}

impl Game {
    pub fn reset(&mut self) {
        let players = self.players.clone();
        let names = self.player_names.clone();
        let scores = self.scores.clone();
        let previous_first = self.current_turn;

        let mut new_game = Game::default();

        new_game.players = players;
        new_game.player_names = names;
        new_game.scores = scores;

        //  Alternate who plays first
        new_game.current_turn = match previous_first {
            Player::X => Player::O,
            Player::O => Player::X,
        };

        *self = new_game;

        debug!(
            "Game reset. New first player: {:?}, Names: {:?}, Scores: {:?}",
            self.current_turn, self.player_names, self.scores
        );
    }

    pub fn make_move(&mut self, player: Player, x: usize, y: usize) -> Result<(), String> {
        if self.game_over {
            debug!("Move rejected: Game is already over.");
            return Err("Game is over!".to_string());
        }
        if self.current_turn != player {
            debug!("Move rejected: Not {:?}'s turn.", player);
            return Err(format!("It's not {:?}'s turn.", player));
        }
        if x >= 3 || y >= 3 {
            debug!("Move rejected: Coordinates out of bounds.");
            return Err("Out of bounds".to_string());
        }
        if self.board[x][y].is_some() {
            debug!("Move rejected: Cell already taken.");
            return Err("Cell already taken".to_string());
        }

        self.board[x][y] = Some(player);

        if self.check_winner().is_some() {
            self.game_over = true;
            *self.scores.entry(player).or_insert(0) += 1; // ✅ Increment score
            debug!("Game over: {:?} wins. Score updated.", player);
        } else if self.is_full() {
            self.game_over = true;
            self.draw = true;
            debug!("Game over: It's a draw.");
        } else {
            self.current_turn = match self.current_turn {
                Player::X => Player::O,
                Player::O => Player::X,
            };
            debug!("Turn switched: Now it's {:?}'s turn.", self.current_turn);
        }

        self.last_activity = SystemTime::now();
        Ok(())
    }

    fn check_winner(&self) -> Option<Player> {
        for i in 0..3 {
            // Check rows and columns for a winner
            if self.board[i][0] == self.board[i][1] && self.board[i][1] == self.board[i][2] {
                if let Some(player) = self.board[i][0] {
                    return Some(player);
                }
            }
            if self.board[0][i] == self.board[1][i] && self.board[1][i] == self.board[2][i] {
                if let Some(player) = self.board[0][i] {
                    return Some(player);
                }
            }
        }

        // Check diagonals for a winner
        if self.board[0][0] == self.board[1][1] && self.board[1][1] == self.board[2][2] {
            if let Some(player) = self.board[0][0] {
                return Some(player);
            }
        }
        if self.board[0][2] == self.board[1][1] && self.board[1][1] == self.board[2][0] {
            if let Some(player) = self.board[0][2] {
                return Some(player);
            }
        }

        None
    }

    fn is_full(&self) -> bool {
        self.board
            .iter()
            .all(|row| row.iter().all(|&cell| cell.is_some()))
    }
}
