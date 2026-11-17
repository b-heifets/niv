use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{config::AtlasStyle, volume::{Axis, Volume}};

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
    pub show_atlas: bool,
    pub show_overlays: bool,
    pub atlas_style: AtlasStyle,
    pub inspect_mode: bool,
    /// In-plane source voxel coordinates; None before any mouse movement.
    pub probe: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action { Quit, Redraw, None }

impl App {
    pub fn new(axis: Axis, index: usize, stride: usize, rotation: u16,
        window: (f32, f32), path: String) -> Self {
        Self {
            axis, index, stride, rotation, window, default_window: window, path,
            flip_h: false, flip_v: false, show_atlas: true, show_overlays: true,
            atlas_style: AtlasStyle::Smooth, inspect_mode: false, probe: None,
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
        self.probe = None;
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


    /// Convert a source voxel center to displayed pixel-grid indices,
    /// accounting for rotation and flips. Useful for keyboard inspection.
    fn oriented(&self, vol: &Volume, h: usize, v: usize) -> (usize, usize, usize, usize) {
        let (sw, sh, _) = vol.plane(self.axis);
        let (mut x, mut y, w, height) = match self.rotation {
            0 => (h, sh - 1 - v, sw, sh),
            90 => (v, h, sh, sw),
            180 => (sw - 1 - h, v, sw, sh),
            270 => (sh - 1 - v, sw - 1 - h, sh, sw),
            _ => unreachable!(),
        };
        if self.flip_h { x = w-1-x; }
        if self.flip_v { y = height-1-y; }
        (x,y,w,height)
    }

    fn move_probe(&mut self, vol: &Volume, dx: isize, dy: isize) {
        let (sw, sh, _) = vol.plane(self.axis);
        let (h,v) = self.probe.unwrap_or((sw/2, sh/2));
        let (x,y,w,height) = self.oriented(vol,h,v);
        let mut x = (x as isize + dx).clamp(0,w as isize-1) as usize;
        let mut y = (y as isize + dy).clamp(0,height as isize-1) as usize;
        if self.flip_h { x=w-1-x; }
        if self.flip_v { y=height-1-y; }
        self.probe=Some(match self.rotation {
            0 => (x,sh-1-y), 90 => (y,x),
            180 => (sw-1-x,y), 270 => (sw-1-y,sh-1-x),
            _ => unreachable!(),
        });
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
            KeyCode::Char('b') => self.show_atlas = !self.show_atlas,
            KeyCode::Char('g') => self.atlas_style = match self.atlas_style {
                AtlasStyle::Smooth => AtlasStyle::Voxel, AtlasStyle::Voxel => AtlasStyle::Smooth,
            },
            KeyCode::Char('i') => {
                self.inspect_mode = !self.inspect_mode;
                if self.inspect_mode && self.probe.is_none() {
                    let (w,h,_) = vol.plane(self.axis);
                    self.probe = Some((w/2,h/2));
                }
            },
            KeyCode::Char('o') => self.show_overlays = !self.show_overlays,
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
                if self.inspect_mode {
                    let (dx,dy) = match key.code {
                        KeyCode::Right | KeyCode::Char('l'|'L') => (1,0),
                        KeyCode::Left | KeyCode::Char('h'|'H') => (-1,0),
                        KeyCode::Up | KeyCode::Char('k'|'K') => (0,-1),
                        _ => (0,1),
                    };
                    self.move_probe(vol, dx * multiplier as isize, dy * multiplier as isize);
                } else {
                    self.move_slices(vol, direction * (multiplier * self.stride) as isize);
                }
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
        Volume { data: Array3::zeros((20,30,100)), spacing: [1.;3], sform: None }
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
    #[test]
    fn keyboard_inspector_preserves_slice_and_tracks_screen_directions() {
        let vol=example();
        let mut app=App::new(Axis::Z,10,1,0,(0.,1.),"test".into());
        app.key(&vol,KeyEvent::new(KeyCode::Char('i'),KeyModifiers::NONE));
        assert!(app.inspect_mode);
        assert_eq!(app.probe,Some((10,15)));
        app.key(&vol,KeyEvent::new(KeyCode::Right,KeyModifiers::NONE));
        assert_eq!(app.probe,Some((11,15)));
        assert_eq!(app.index,10);
        app.key(&vol,KeyEvent::new(KeyCode::Up,KeyModifiers::NONE));
        assert_eq!(app.probe,Some((11,16))); // screen up = higher source y at rot 0
        app.key(&vol,KeyEvent::new(KeyCode::PageUp,KeyModifiers::NONE));
        assert_eq!(app.index,35);
        app.key(&vol,KeyEvent::new(KeyCode::Char('i'),KeyModifiers::NONE));
        assert!(!app.inspect_mode);
        app.key(&vol,KeyEvent::new(KeyCode::Right,KeyModifiers::NONE));
        assert_eq!(app.index,36);
    }
}
