use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::volume::{Axis, Volume};

pub struct App {
    pub axis: Axis,
    pub index: usize,
    pub step: usize,
    pub window: (f32, f32),
    pub default_window: (f32, f32),
    pub path: String,
    pub flip_h: bool,
    pub flip_v: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action { Quit, Redraw, None }

impl App {
    pub fn new(axis: Axis, index: usize, step: usize, window: (f32, f32), path: String) -> Self {
        Self { axis, index, step, window, default_window: window, path, flip_h: false, flip_v: false }
    }

    fn set_axis(&mut self, vol: &Volume, axis: Axis) {
        if self.axis == axis { return; }
        let old_len = vol.depth(self.axis);
        let new_len = vol.depth(axis);
        let fraction = if old_len > 1 { self.index as f64 / (old_len - 1) as f64 } else { 0.5 };
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
            KeyCode::Char('f') => self.flip_h = !self.flip_h,
            KeyCode::Char('v') => self.flip_v = !self.flip_v,
            KeyCode::Char('0') => self.window = self.default_window,
            KeyCode::Char('+') | KeyCode::Char('=') => self.scale_window(0.8),
            KeyCode::Char('-') | KeyCode::Char('_') => self.scale_window(1.25),
            KeyCode::Char(',') => self.shift_window(-0.1),
            KeyCode::Char('.') => self.shift_window(0.1),
            KeyCode::Char('[') => self.step = (self.step / 2).max(1),
            KeyCode::Char(']') => self.step = self.step.saturating_mul(2).min(256),
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
                let multiplier = if m.contains(KeyModifiers::ALT) { 50 }
                    else if m.contains(KeyModifiers::CONTROL) { 25 }
                    else if m.contains(KeyModifiers::SHIFT)
                        || matches!(key.code, KeyCode::Char('H'|'J'|'K'|'L')) { 10 }
                    else { 1 };
                self.move_slices(vol, direction * (multiplier * self.step) as isize);
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

    #[test]
    fn test_key_navigation_and_window() {
        let vol = Volume { data: Array3::zeros((20,30,100)), spacing: [1.;3] };
        let mut app = App::new(Axis::Z, 50, 1, (0.,100.), "test".into());
        app.key(&vol, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.index, 51);
        app.key(&vol, KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
        assert_eq!(app.index, 41);
        app.key(&vol, KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(app.index, 66);
        app.key(&vol, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.axis, Axis::Y);
        assert!(app.index < vol.depth(Axis::Y));
        app.key(&vol, KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert!(app.window.1 - app.window.0 < 100.);
    }
}
