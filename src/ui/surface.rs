//! Двумерная Surface буферизация окон и оверлеев с поддержкой Damage Region

use crate::renderer::Rect;

#[derive(Clone, Debug)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub damage: Option<Rect>,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Self {
        Surface {
            width,
            height,
            damage: Some(Rect::new(0, 0, width, height)),
        }
    }

    pub fn mark_dirty(&mut self, rect: Rect) {
        self.damage = match self.damage {
            Some(existing) => Some(existing.union(&rect)),
            None => Some(rect),
        };
    }

    pub fn clear_damage(&mut self) {
        self.damage = None;
    }
}
