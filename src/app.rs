use crate::{
    layers::{Layers, Palette},
    volume::{Axis, Volume},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditField {
    Both,
    Minimum,
    Maximum,
}

pub struct WindowEdit {
    pub field: EditField,
    pub text: String,
    pub error: Option<String>,
}

pub struct App {
    pub axis: Axis,
    pub index: usize,
    pub stride: usize,
    pub rotation: u16,
    pub window: (f32, f32),
    pub default_window: (f32, f32),
    pub base_palette: Palette,
    pub path: String,
    pub atlas_name: Option<String>,
    pub selected: usize, // 0 = base; subsequent indices = overlay layers
    pub flip_h: bool,
    pub flip_v: bool,
    pub show_atlas: bool,
    pub show_overlays: bool,
    pub fullscreen: bool,
    pub show_layer_list: bool,
    pub inspect_mode: bool,
    pub probe: Option<(usize, usize)>,
    pub edit: Option<WindowEdit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Redraw,
    Ui,
    Layout,
    None,
}

impl App {
    pub fn new(
        axis: Axis,
        index: usize,
        stride: usize,
        rotation: u16,
        window: (f32, f32),
        path: String,
    ) -> Self {
        Self {
            axis,
            index,
            stride,
            rotation,
            window,
            default_window: window,
            base_palette: Palette::Gray,
            path,
            atlas_name: None,
            selected: 0,
            flip_h: false,
            flip_v: false,
            show_atlas: true,
            show_overlays: true,
            fullscreen: false,
            show_layer_list: false,
            inspect_mode: false,
            probe: None,
            edit: None,
        }
    }

    pub fn selected_window(&self, layers: &Layers) -> (f32, f32) {
        if self.selected == 0 {
            self.window
        } else {
            layers.overlays[self.selected - 1].window
        }
    }

    fn set_window(&mut self, layers: &mut Layers, window: (f32, f32)) {
        if self.selected == 0 {
            self.window = window;
        } else {
            layers.overlays[self.selected - 1].window = window;
        }
    }

    fn default_selected_window(&self, layers: &Layers) -> (f32, f32) {
        if self.selected == 0 {
            self.default_window
        } else {
            layers.overlays[self.selected - 1].default_window
        }
    }

    pub fn selected_palette(&self, layers: &Layers) -> Palette {
        if self.selected == 0 {
            self.base_palette
        } else {
            layers.overlays[self.selected - 1].palette
        }
    }

    fn cycle_palette(&mut self, layers: &mut Layers) {
        if self.selected == 0 {
            self.base_palette = self.base_palette.next();
        } else {
            let layer = &mut layers.overlays[self.selected - 1];
            layer.palette = layer.palette.next();
        }
    }

    fn scale_window(&mut self, layers: &mut Layers, ratio: f32) {
        let (lo, hi) = self.selected_window(layers);
        let mid = lo * 0.5 + hi * 0.5;
        let half = ((hi - lo) * 0.5 * ratio).clamp(1e-7, 1e20);
        self.set_window(layers, (mid - half, mid + half));
    }

    fn shift_window(&mut self, layers: &mut Layers, fraction: f32) {
        let (lo, hi) = self.selected_window(layers);
        let delta = (hi - lo) * fraction;
        self.set_window(layers, (lo + delta, hi + delta));
    }

    fn set_axis(&mut self, vol: &Volume, axis: Axis) {
        if self.axis == axis {
            return;
        }
        let old_len = vol.depth(self.axis);
        let new_len = vol.depth(axis);
        let fraction = if old_len > 1 {
            self.index as f64 / (old_len - 1) as f64
        } else {
            0.5
        };
        self.axis = axis;
        self.probe = None;
        self.index = (fraction * (new_len - 1) as f64).round() as usize;
    }

    pub fn move_slices(&mut self, vol: &Volume, delta: isize) -> bool {
        let new_index =
            (self.index as isize + delta).clamp(0, vol.depth(self.axis) as isize - 1) as usize;
        let changed = new_index != self.index;
        self.index = new_index;
        changed
    }

    pub fn multiplier(m: KeyModifiers, uppercase: bool) -> usize {
        if m.contains(KeyModifiers::ALT) {
            4
        } else if m.contains(KeyModifiers::CONTROL) {
            3
        } else if m.contains(KeyModifiers::SHIFT) || uppercase {
            2
        } else {
            1
        }
    }

    fn oriented(&self, vol: &Volume, h: usize, v: usize) -> (usize, usize, usize, usize) {
        let (sw, sh, _) = vol.plane(self.axis);
        let (mut x, mut y, w, height) = match self.rotation {
            0 => (h, sh - 1 - v, sw, sh),
            90 => (v, h, sh, sw),
            180 => (sw - 1 - h, v, sw, sh),
            270 => (sh - 1 - v, sw - 1 - h, sh, sw),
            _ => unreachable!(),
        };
        if self.flip_h {
            x = w - 1 - x;
        }
        if self.flip_v {
            y = height - 1 - y;
        }
        (x, y, w, height)
    }

    fn move_probe(&mut self, vol: &Volume, dx: isize, dy: isize) {
        let (sw, sh, _) = vol.plane(self.axis);
        let (h, v) = self.probe.unwrap_or((sw / 2, sh / 2));
        let (x, y, w, height) = self.oriented(vol, h, v);
        let mut x = (x as isize + dx).clamp(0, w as isize - 1) as usize;
        let mut y = (y as isize + dy).clamp(0, height as isize - 1) as usize;
        if self.flip_h {
            x = w - 1 - x;
        }
        if self.flip_v {
            y = height - 1 - y;
        }
        self.probe = Some(match self.rotation {
            0 => (x, sh - 1 - y),
            90 => (y, x),
            180 => (sw - 1 - x, y),
            270 => (sw - 1 - y, sh - 1 - x),
            _ => unreachable!(),
        });
    }

    fn edit_key(&mut self, layers: &mut Layers, key: KeyCode) -> Action {
        let Some(editor) = self.edit.as_mut() else {
            return Action::None;
        };
        match key {
            KeyCode::Esc => {
                self.edit = None;
                Action::Ui
            }
            KeyCode::Enter => {
                let (field, text) = (editor.field, editor.text.trim().to_owned());
                let current = self.selected_window(layers);
                let result = parse_edit(field, &text, current);
                match result {
                    Ok(window) => {
                        self.set_window(layers, window);
                        self.edit = None;
                        Action::Redraw
                    }
                    Err(error) => {
                        if let Some(editor) = self.edit.as_mut() {
                            editor.error = Some(error);
                        }
                        Action::Ui
                    }
                }
            }
            KeyCode::Backspace => {
                editor.text.pop();
                editor.error = None;
                Action::Ui
            }
            KeyCode::Char(c)
                if (c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | '+' | 'e' | 'E'))
                    && editor.text.len() < 64 =>
            {
                editor.text.push(c);
                editor.error = None;
                Action::Ui
            }
            _ => Action::None,
        }
    }

    pub fn key(&mut self, vol: &Volume, layers: &mut Layers, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        let m = key.modifiers;
        if self.edit.is_some() {
            return self.edit_key(layers, key.code);
        }
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && m.contains(KeyModifiers::CONTROL))
        {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Tab => {
                self.fullscreen = !self.fullscreen;
                Action::Layout
            }
            KeyCode::Char('t') => {
                self.show_layer_list = !self.show_layer_list;
                Action::Layout
            }
            KeyCode::Char(c @ '1'..='9') => {
                let selected = (c as u8 - b'1') as usize;
                if selected <= layers.overlays.len() {
                    self.selected = selected;
                    Action::Ui
                } else {
                    Action::None
                }
            }
            KeyCode::Char('e') | KeyCode::Char('m') | KeyCode::Char('M') => {
                let field = match key.code {
                    KeyCode::Char('m') => EditField::Minimum,
                    KeyCode::Char('M') => EditField::Maximum,
                    _ => EditField::Both,
                };
                self.edit = Some(WindowEdit {
                    field,
                    text: String::new(),
                    error: None,
                });
                Action::Ui
            }
            KeyCode::Char('c') => {
                self.cycle_palette(layers);
                Action::Redraw
            }
            KeyCode::Char(' ') => {
                if self.selected > 0 {
                    let layer = &mut layers.overlays[self.selected - 1];
                    layer.visible = !layer.visible;
                    Action::Redraw
                } else {
                    Action::None
                }
            }
            KeyCode::Char('a') => {
                self.show_atlas = !self.show_atlas;
                Action::Redraw
            }
            KeyCode::Char('x') => {
                self.set_axis(vol, Axis::X);
                Action::Redraw
            }
            KeyCode::Char('y') => {
                self.set_axis(vol, Axis::Y);
                Action::Redraw
            }
            KeyCode::Char('z') => {
                self.set_axis(vol, Axis::Z);
                Action::Redraw
            }
            KeyCode::Char('r') => {
                self.rotation = (self.rotation + 90) % 360;
                Action::Redraw
            }
            KeyCode::Char('R') => {
                self.rotation = (self.rotation + 270) % 360;
                Action::Redraw
            }
            KeyCode::Char('f') => {
                self.flip_h = !self.flip_h;
                Action::Redraw
            }
            KeyCode::Char('v') => {
                self.flip_v = !self.flip_v;
                Action::Redraw
            }
            KeyCode::Char('o') => {
                self.show_overlays = !self.show_overlays;
                Action::Redraw
            }
            KeyCode::Char('i') => {
                self.inspect_mode = !self.inspect_mode;
                if self.inspect_mode && self.probe.is_none() {
                    let (w, h, _) = vol.plane(self.axis);
                    self.probe = Some((w / 2, h / 2));
                }
                Action::Redraw
            }
            KeyCode::Char('0') => {
                self.set_window(layers, self.default_selected_window(layers));
                Action::Redraw
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.scale_window(layers, 0.8);
                Action::Redraw
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.scale_window(layers, 1.25);
                Action::Redraw
            }
            KeyCode::Char(',') => {
                self.shift_window(layers, -0.1);
                Action::Redraw
            }
            KeyCode::Char('.') => {
                self.shift_window(layers, 0.1);
                Action::Redraw
            }
            KeyCode::Char('[') => {
                self.stride = self.stride.saturating_sub(1).max(1);
                Action::Ui
            }
            KeyCode::Char(']') => {
                self.stride = self.stride.saturating_add(1).min(256);
                Action::Ui
            }
            KeyCode::PageUp => {
                self.move_slices(vol, 25);
                Action::Redraw
            }
            KeyCode::PageDown => {
                self.move_slices(vol, -25);
                Action::Redraw
            }
            KeyCode::Home => {
                self.index = 0;
                Action::Redraw
            }
            KeyCode::End => {
                self.index = vol.depth(self.axis) - 1;
                Action::Redraw
            }
            KeyCode::Up
            | KeyCode::Right
            | KeyCode::Left
            | KeyCode::Down
            | KeyCode::Char('h' | 'j' | 'k' | 'l' | 'H' | 'J' | 'K' | 'L') => {
                let multiplier =
                    Self::multiplier(m, matches!(key.code, KeyCode::Char('H' | 'J' | 'K' | 'L')));
                if self.inspect_mode {
                    let (dx, dy) = match key.code {
                        KeyCode::Right | KeyCode::Char('l' | 'L') => (1, 0),
                        KeyCode::Left | KeyCode::Char('h' | 'H') => (-1, 0),
                        KeyCode::Up | KeyCode::Char('k' | 'K') => (0, -1),
                        _ => (0, 1),
                    };
                    self.move_probe(vol, dx * multiplier as isize, dy * multiplier as isize);
                } else {
                    let sign = match key.code {
                        KeyCode::Up | KeyCode::Right | KeyCode::Char('k' | 'l' | 'K' | 'L') => 1,
                        _ => -1,
                    };
                    self.move_slices(vol, sign * (self.stride * multiplier) as isize);
                }
                Action::Redraw
            }
            _ => Action::None,
        }
    }
}

fn parse_edit(field: EditField, text: &str, current: (f32, f32)) -> Result<(f32, f32), String> {
    let parse = |s: &str| -> Result<f32, String> {
        let n: f32 = s
            .trim()
            .parse()
            .map_err(|_| "Enter a valid number".to_owned())?;
        if n.is_finite() {
            Ok(n)
        } else {
            Err("Enter a finite number".to_owned())
        }
    };
    let (lo, hi) = match field {
        EditField::Both => {
            let (lo, hi) = text
                .split_once(',')
                .ok_or_else(|| "Use min,max (comma-separated)".to_owned())?;
            (parse(lo)?, parse(hi)?)
        }
        EditField::Minimum => (parse(text)?, current.1),
        EditField::Maximum => (current.0, parse(text)?),
    };
    if lo >= hi {
        return Err("Display minimum must be less than maximum".to_owned());
    }
    Ok((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ndarray::Array3;
    fn example() -> Volume {
        Volume {
            data: Array3::zeros((20, 30, 100)),
            spacing: [1.; 3],
            sform: None,
        }
    }
    fn key(
        app: &mut App,
        vol: &Volume,
        layers: &mut Layers,
        code: KeyCode,
        mods: KeyModifiers,
    ) -> Action {
        app.key(vol, layers, KeyEvent::new(code, mods))
    }
    #[test]
    fn modifiers_stride_and_new_atlas_toggle() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 50, 1, 0, (0., 100.), "test".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.index, 51);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('H'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.index, 49); // shift -2
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Right,
            KeyModifiers::CONTROL,
        );
        assert_eq!(app.index, 52); // control +3
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Left,
            KeyModifiers::ALT,
        );
        assert_eq!(app.index, 48); // alt -4
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char(']'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.stride, 2);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char(']'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.stride, 3);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('['),
            KeyModifiers::NONE,
        );
        assert_eq!(app.stride, 2);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        );
        assert!(!app.show_atlas);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Tab,
            KeyModifiers::NONE,
        );
        assert!(app.fullscreen);
    }
    #[test]
    fn edit_window_and_inspection() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 10, 1, 0, (0., 100.), "test".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('e'),
            KeyModifiers::NONE,
        );
        for ch in "-2.5,25".chars() {
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            );
        }
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        assert_eq!(app.window, (-2.5, 25.));
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Right,
            KeyModifiers::NONE,
        );
        assert_eq!(app.probe, Some((11, 15)));
        assert_eq!(app.index, 10);
    }
    #[test]
    fn layer_selection_edits_only_selected_window() {
        use crate::layers::IntensityOverlay;
        let vol = example();
        let mut layers = Layers {
            overlays: vec![IntensityOverlay {
                vol: example(),
                window: (2., 6.),
                default_window: (2., 6.),
                data_range: (0., 9.),
                palette: Palette::Solid([255, 0, 0]),
                visible: true,
                opacity: 0.8,
                name: "overlay".into(),
            }],
            atlas: None,
        };
        let mut app = App::new(Axis::Z, 10, 1, 0, (0., 100.), "base".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('2'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.selected, 1);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('m'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('3'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        assert_eq!(layers.overlays[0].window, (3., 6.));
        assert_eq!(app.window, (0., 100.));
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        );
        assert_eq!(layers.overlays[0].palette, Palette::Gray);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        );
        assert!(!layers.overlays[0].visible);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('0'),
            KeyModifiers::NONE,
        );
        assert_eq!(layers.overlays[0].window, (2., 6.));
    }
    #[test]
    fn editor_validation() {
        assert_eq!(
            parse_edit(EditField::Minimum, "4", (0., 10.)).unwrap(),
            (4., 10.)
        );
        assert!(parse_edit(EditField::Both, "5,1", (0., 10.)).is_err());
        assert!(parse_edit(EditField::Maximum, "NaN", (0., 10.)).is_err());
    }
}
