use rand::seq::SliceRandom; // For random AI moves

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    X,
    O,
}

// Manually implement the Default trait for Player
impl Default for Player {
    fn default() -> Self {
        Player::X // Default to Player X
    }
}

#[derive(Debug, Default)]
pub struct Game {
    pub board: [[Option<Player>; 3]; 3], // 3x3 game board
    pub current_turn: Player,            // Current player's turn
}

impl Game {
    // Initialize a new game with a default state
    pub fn new() -> Self {
        Self::default()
    }

    // Check if the board is full
    pub fn is_full(&self) -> bool {
        self.board
            .iter()
            .all(|row| row.iter().all(|&cell| cell.is_some()))
    }

    // Make a move at the specified (x, y) position
    pub fn make_move(&mut self, x: usize, y: usize) -> Result<(), String> {
        if x >= 3 || y >= 3 {
            return Err("Invalid move: out of bounds".to_string());
        }
        if self.board[x][y].is_some() {
            return Err("Invalid move: cell already taken".to_string());
        }

        // Place the current player's move on the board
        self.board[x][y] = Some(self.current_turn);

        // Check for a winner after placing the move
        if self.check_winner().is_some() {
            return Ok(()); // Game is over, winner is decided
        }

        // Switch turn if no winner yet
        self.current_turn = match self.current_turn {
            Player::X => Player::O,
            Player::O => Player::X,
        };

        Ok(())
    }

    // Check for a winner
    pub fn check_winner(&self) -> Option<Player> {
        Self::check_winner_with_board(&self.board)
    }

    fn check_winner_with_board(board: &[[Option<Player>; 3]; 3]) -> Option<Player> {
        for i in 0..3 {
            // Check rows and columns
            if board[i][0] == board[i][1] && board[i][1] == board[i][2] {
                if let Some(player) = board[i][0] {
                    return Some(player);
                }
            }
            if board[0][i] == board[1][i] && board[1][i] == board[2][i] {
                if let Some(player) = board[0][i] {
                    return Some(player);
                }
            }
        }
        // Check diagonals
        if board[0][0] == board[1][1] && board[1][1] == board[2][2] {
            if let Some(player) = board[0][0] {
                return Some(player);
            }
        }
        if board[0][2] == board[1][1] && board[1][1] == board[2][0] {
            if let Some(player) = board[0][2] {
                return Some(player);
            }
        }
        None
    }

    // AI logic for making a move
    pub fn ai_move(&mut self) -> Result<(), String> {
        if let Some((x, y)) = self.find_winning_move(Player::O) {
            self.make_move(x, y)?;
        } else if let Some((x, y)) = self.find_winning_move(Player::X) {
            self.make_move(x, y)?;
        } else if self.board[1][1].is_none() {
            self.make_move(1, 1)?;
        } else {
            let corners = [(0, 0), (0, 2), (2, 0), (2, 2)];
            for &(x, y) in &corners {
                if self.board[x][y].is_none() {
                    self.make_move(x, y)?;
                    return Ok(());
                }
            }
            let empty_cells: Vec<(usize, usize)> = (0..3)
                .flat_map(|x| (0..3).map(move |y| (x, y)))
                .filter(|&(x, y)| self.board[x][y].is_none())
                .collect();
            if let Some(&(x, y)) = empty_cells.choose(&mut rand::thread_rng()) {
                self.make_move(x, y)?;
            }
        }
        Ok(())
    }

    // Find a winning move for a player
    fn find_winning_move(&self, player: Player) -> Option<(usize, usize)> {
        for x in 0..3 {
            for y in 0..3 {
                if self.board[x][y].is_none() {
                    let mut board_copy = self.board;
                    board_copy[x][y] = Some(player);
                    if Self::check_winner_with_board(&board_copy) == Some(player) {
                        return Some((x, y));
                    }
                }
            }
        }
        None
    }
}
