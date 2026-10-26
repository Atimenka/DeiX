//! Пасхалка 2048: логика и отрисовка прямо в окне ОС.
//!
//! Игрок двигает плитки стрелками (или WASD). При слиянии двух плиток
//! одного числа удваивается значение и добавляется в счёт. Цель — получить
//! плитку 2048. После каждого хода появляется новая плитка (2 или 4). Игра
//! окончена, если плитки заполнились и ни одна из них не может слиться.
//!
//! Логика направлений через транспонирование/инверсию строк:
//! вправо — инвертируем строку, применяем слияние влево, инвертируем обратно;
//! вниз — инвертируем колонку, сливаем вверх, инвертируем обратно.

use crate::renderer::{Color, Renderer};
use crate::ui::theme::UiTheme;
use crate::ui::window::Window;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

pub const SIZE: usize = 4;

pub struct Game2048 {
    grid: [u32; SIZE * SIZE],
    score: u32,
    won: bool,
    lost: bool,
    rng_state: u32,
}

#[derive(Clone, Copy)]
pub enum MoveDir {
    Up,
    Down,
    Left,
    Right,
}

impl Game2048 {
    pub fn new() -> Self {
        let mut g = Game2048 {
            grid: [0; SIZE * SIZE],
            score: 0,
            won: false,
            lost: false,
            rng_state: 0x2545F491,
        };
        g.spawn();
        g.spawn();
        g
    }

    fn next_rand(&mut self) -> u32 {
        // LCG — быстрый, безвлечений в драйверы RNG.
        self.rng_state = self
            .rng_state
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
        self.rng_state >> 1
    }

    fn empty_cells(&self) -> Vec<usize> {
        (0..SIZE * SIZE).filter(|i| self.grid[*i] == 0).collect()
    }

    fn spawn(&mut self) {
        let cells = self.empty_cells();
        if cells.is_empty() {
            return;
        }
        let idx = (self.next_rand() as usize) % cells.len();
        self.grid[cells[idx]] = if self.next_rand() % 10 == 0 { 4 } else { 2 };
    }

    /// Сливаем строку влево: собираем ненулевые к старту, объединяем
    /// одинаковые соседи (каждый раз только одна пара). Возвращает,
    /// изменилась ли строка, и сколько очков набрано.
    fn apply_left(line: &mut [u32; SIZE]) -> (bool, u32) {
        let mut target = 0usize;
        let mut merged = [false; SIZE];
        let mut gained = 0u32;
        let mut out = [0u32; SIZE];
        for i in 0..SIZE {
            let val = line[i];
            if val == 0 {
                continue;
            }
            if target == 0 {
                out[0] = val;
                target = 1;
            } else {
                let prev = out[target - 1];
                if prev == val && !merged[target - 1] {
                    out[target - 1] = val * 2;
                    merged[target - 1] = true;
                    gained += val * 2;
                } else {
                    out[target] = val;
                    target += 1;
                }
            }
        }
        for i in target..SIZE {
            out[i] = 0;
        }
        // Сравнить с исходной строкой, затем записать результат обратно.
        let mut result = false;
        for i in 0..SIZE {
            if out[i] != line[i] {
                result = true;
            }
            line[i] = out[i];
        }
        (result, gained)
    }

    fn move_left(&mut self) -> bool {
        if self.won || self.lost {
            return false;
        }
        let mut changed = false;
        for r in 0..SIZE {
            let mut line = [0u32; SIZE];
            for c in 0..SIZE {
                line[c] = self.grid[r * SIZE + c];
            }
            let (rchanged, gained) = Game2048::apply_left(&mut line);
            if rchanged {
                changed = true;
                self.score += gained;
            }
            for c in 0..SIZE {
                self.grid[r * SIZE + c] = line[c];
            }
        }
        if changed {
            self.spawn();
        }
        changed
    }

    fn move_right(&mut self) -> bool {
        if self.won || self.lost {
            return false;
        }
        let mut changed = false;
        for r in 0..SIZE {
            let mut line = [0u32; SIZE];
            for c in 0..SIZE {
                line[c] = self.grid[r * SIZE + c];
            }
            // Инвертируем строку, сливаем влево, инвертируем обратно.
            for c in 0..SIZE / 2 {
                line.swap(c, SIZE - 1 - c);
            }
            let (rchanged, gained) = Game2048::apply_left(&mut line);
            if rchanged {
                changed = true;
                self.score += gained;
            }
            for c in 0..SIZE / 2 {
                line.swap(c, SIZE - 1 - c);
            }
            for c in 0..SIZE {
                self.grid[r * SIZE + c] = line[c];
            }
        }
        if changed {
            self.spawn();
        }
        changed
    }

    fn move_up(&mut self) -> bool {
        if self.won || self.lost {
            return false;
        }
        let mut changed = false;
        for c in 0..SIZE {
            let mut col = [0u32; SIZE];
            for r in 0..SIZE {
                col[r] = self.grid[r * SIZE + c];
            }
            // Сливаем вверх: как строка влево.
            let (rchanged, gained) = Game2048::apply_left(&mut col);
            if rchanged {
                changed = true;
                self.score += gained;
            }
            for r in 0..SIZE {
                self.grid[r * SIZE + c] = col[r];
            }
        }
        if changed {
            self.spawn();
        }
        changed
    }

    fn move_down(&mut self) -> bool {
        if self.won || self.lost {
            return false;
        }
        let mut changed = false;
        for c in 0..SIZE {
            let mut col = [0u32; SIZE];
            for r in 0..SIZE {
                col[r] = self.grid[r * SIZE + c];
            }
            // Инвертируем колонку, сливаем вверх, инвертируем обратно.
            for r in 0..SIZE / 2 {
                col.swap(r, SIZE - 1 - r);
            }
            let (rchanged, gained) = Game2048::apply_left(&mut col);
            if rchanged {
                changed = true;
                self.score += gained;
            }
            for r in 0..SIZE / 2 {
                col.swap(r, SIZE - 1 - r);
            }
            for r in 0..SIZE {
                self.grid[r * SIZE + c] = col[r];
            }
        }
        if changed {
            self.spawn();
        }
        changed
    }

    pub fn handle_move(&mut self, dir: MoveDir) -> bool {
        let moved = match dir {
            MoveDir::Up => self.move_up(),
            MoveDir::Down => self.move_down(),
            MoveDir::Left => self.move_left(),
            MoveDir::Right => self.move_right(),
        };
        if moved {
            for &v in self.grid.iter() {
                if v == 2048 {
                    self.won = true;
                    break;
                }
            }
            if !self.won && self.lost_check() {
                self.lost = true;
            }
        }
        moved
    }

    fn lost_check(&self) -> bool {
        // Нет пустых клеток и нет соседних одинаковых.
        for i in 0..SIZE * SIZE {
            if self.grid[i] == 0 {
                return false;
            }
            let r = i / SIZE;
            let c = i % SIZE;
            if c + 1 < SIZE && self.grid[i] == self.grid[r * SIZE + c + 1] {
                return false;
            }
            if r + 1 < SIZE && self.grid[i] == self.grid[(r + 1) * SIZE + c] {
                return false;
            }
        }
        true
    }

    pub fn new_game(&mut self) {
        *self = Game2048::new();
    }

    pub fn won(&self) -> bool {
        self.won
    }

    pub fn lost(&self) -> bool {
        self.lost
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    pub fn grid(&self) -> &[u32; SIZE * SIZE] {
        &self.grid
    }
}

// ---------------------------------------------------------------- цвет

fn tile_color(val: u32) -> Color {
    match val {
        0 => Color::rgb(40, 44, 52),
        2 => Color::rgb(238, 228, 218),
        4 => Color::rgb(237, 224, 200),
        8 => Color::rgb(242, 177, 121),
        16 => Color::rgb(245, 149, 99),
        32 => Color::rgb(246, 124, 95),
        64 => Color::rgb(246, 94, 59),
        128 => Color::rgb(237, 207, 114),
        256 => Color::rgb(237, 204, 97),
        512 => Color::rgb(237, 200, 80),
        1024 => Color::rgb(237, 197, 63),
        2048 => Color::rgb(237, 194, 46),
        _ => Color::rgb(60, 58, 50),
    }
}

fn text_color_for(val: u32) -> Color {
    match val {
        2 | 4 | 8 | 16 => Color::rgb(119, 110, 101),
        _ => Color::WHITE,
    }
}

// ---------------------------------------------------------------- отрисовка

pub fn draw_game2048(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    game: &Game2048,
    content_y: i32,
) {
    r.fill_rect_alpha(w.x, content_y, w.width, w.height, theme.window_bg, theme.opacity);

    r.draw_text(w.x + 12, content_y + 8, "2048", theme.accent, None);
    r.draw_text(
        w.x + w.width as i32 - 120, content_y + 8,
        &format!("SCORE: {}", game.score()),
        theme.text_primary, None,
    );

    if game.won() {
        r.draw_text(w.x + 12, content_y + 28, "You reached 2048!", Color::rgb(230, 160, 60), None);
    }
    if game.lost() {
        r.draw_text(w.x + 12, content_y + 28, "Game over", Color::rgb(220, 60, 60), None);
    }

    // Сетка.
    let grid_x = w.x + 16;
    let grid_y = content_y + 52;
    let cell: i32 = 64;
    let gap: i32 = 8;
    let step = cell + gap;
    let grid_w = (step * SIZE as i32 - gap) as u32;

    // Подложка.
    r.fill_rounded_rect(grid_x, grid_y, grid_w, grid_w, 8, theme.titlebar_inactive);

    for i in 0..SIZE * SIZE {
        let row = i / SIZE;
        let col = i % SIZE;
        let cx = grid_x + 4 + col as i32 * step;
        let cy = grid_y + 4 + row as i32 * step;
        let val = game.grid()[i];
        r.fill_rounded_rect(cx, cy, cell as u32, cell as u32, 6, tile_color(val));
        if val > 0 {
            let txt = val.to_string();
            let tw = txt.chars().count() as i32 * 7;
            r.draw_text(
                cx + (cell - tw) / 2,
                cy + cell / 2 - 8,
                &txt,
                text_color_for(val),
                None,
            );
        }
    }

    r.draw_text(w.x + 12, content_y + w.height as i32 - 24, "Arrows / WASD to move", theme.text_secondary, None);
    r.draw_text(w.x + 180, content_y + w.height as i32 - 24, "R — new game", theme.text_secondary, None);
}
