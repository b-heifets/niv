use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::volume::{Axis, Volume};

pub struct App {
    pub axis: Axis,
    pub index: usize,
    /// Base slice increment. Modifiers multiply this value.
    pub stride: usize,
    pub rotation: u16,
    pub window: (f32, f32),
    pub default_window: (f32, f32),
    pub path: String,
    pub flip_h: bool,
    pub flip_v: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action { Quit, Redraw, None }

impl App {
    pub fn new(axis: Axis, index: usize, stride: usize, rotation: u16,
        window: (f32, f32), path: String) -> Self {
        Self {
            axis, index, stride, rotation, window, default_window: window, path,
            flip_h: false, flip_v: false,
        }
    }

    fn set_axis(&mut self, vol: &Volume, axis: Axis) {
        if self.axis == axis { return; }
        let old_len = vol.depth(self.axis);
        let new_len = vol.depth(axis);
        let fraction = if old_len > 1 {
            self.index as f64 / (old_len - 1) as f64
        } else { 0.5 };
        self.axis = axis;
        self.index = (fraction * (new_len - 1) as f64).round() as usize;
    }

    fn move_slices(&mut self, vol: &Volume, offset: isize) {
        self.index = (self.index as isize + offset)
            .clamp(0, vol.depth(self.axis) as isize - 1) as usize;
    }

    fn scale_window(&mut self, ratio: f32) {
        let (lo, hi) = self.window;
        let mid = lo * 0.5 + hi * 0.5;
        let half = ((hi - lo) * 0.5 * ratio).clamp(1e-7, 1e20);
        self.window = (mid - half, mid + half);
    }

    fn shift_window(&mut self, fraction: f32) {
        let (lo, hi) = self.window;
        let move_by = (hi - lo) * fraction;
        self.window = (lo + move_by, hi + move_by);
    }

    pub fn key(&mut self, vol: &Volume, key: KeyEvent) -> Action {
        // Enhanced keyboard mode can report presses, repeats, and releases.
        if key.kind == KeyEventKind::Release { return Action::None; }
        let m = key.modifiers;
        if key.code == KeyCode::Esc || key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && m.contains(KeyModifiers::CONTROL)) {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Char('a') => self.set_axis(vol, self.axis.cycle()),
            KeyCode::Char('x') => self.set_axis(vol, Axis::X),
            KeyCode::Char('y') => self.set_axis(vol, Axis::Y),
            KeyCode::Char('z') => self.set_axis(vol, Axis::Z),
            KeyCode::Char('r') => self.rotation = (self.rotation + 90) % 360,
            KeyCode::Char('R') => self.rotation = (self.rotation + 270) % 360,
            KeyCode::Char('f') => self.flip_h = !self.flip_h,
            KeyCode::Char('v') => self.flip_v = !self.flip_v,
            KeyCode::Char('0') => self.window = self.default_window,
            KeyCode::Char('+') | KeyCode::Char('=') => self.scale_window(0.8),
            KeyCode::Char('-') | KeyCode::Char('_') => self.scale_window(1.25),
            KeyCode::Char(',') => self.shift_window(-0.1),
            KeyCode::Char('.') => self.shift_window(0.1),
            KeyCode::Char('[') => self.stride = (self.stride / 2).max(1),
            KeyCode::Char(']') => self.stride = self.stride.saturating_mul(2).min(256),
            KeyCode::PageUp => self.move_slices(vol, 25),
            KeyCode::PageDown => self.move_slices(vol, -25),
            KeyCode::Home => self.index = 0,
            KeyCode::End => self.index = vol.depth(self.axis) - 1,
            KeyCode::Up | KeyCode::Right | KeyCode::Left | KeyCode::Down
            | KeyCode::Char('h' | 'j' | 'k' | 'l' | 'H' | 'J' | 'K' | 'L') => {
                let direction: isize = match key.code {
                    KeyCode::Up | KeyCode::Right | KeyCode::Char('k'|'l'|'K'|'L') => 1,
                    _ => -1,
                };
                let multiplier = if m.contains(KeyModifiers::CONTROL) { 10 }
                    else if m.contains(KeyModifiers::SHIFT)
                        || matches!(key.code, KeyCode::Char('H'|'J'|'K'|'L')) { 5 }
                    else if m.contains(KeyModifiers::ALT) { 2 }
                    else { 1 };
                self.move_slices(vol, direction * (multiplier * self.stride) as isize);
            }
            _ => return Action::None,
        }
        Action::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ndarray::Array3;

    fn example() -> Volume {
        Volume { data: Array3::zeros((20,30,100)), spacing: [1.;3] }
    }
    #[test]
    fn navigation_rotation_and_window() {
        let vol = example();
        let mut app = App::new(Axis::Z, 50, 1, 0, (0.,100.), "test".into());
        app.key(&vol, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.index, 51);
        app.key(&vol, KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
        assert_eq!(app.index, 46); // Shift 5x, negative
        app.key(&vol, KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(app.index, 56); // Ctrl 10x
        app.key(&vol, KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(app.index, 54); // Alt 2x
        app.key(&vol, KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        assert_eq!(app.stride, 2);
        app.key(&vol, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.index, 56);
        app.key(&vol, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(app.rotation, 90);
        app.key(&vol, KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
        assert_eq!(app.rotation, 0);
        app.key(&vol, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.axis, Axis::Y);
        assert!(app.index < vol.depth(Axis::Y));
        app.key(&vol, KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert!(app.window.1 - app.window.0 < 100.);
    }
}
