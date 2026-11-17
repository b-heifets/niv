use crate::path_complete::{self, PathMatches};
use crate::{
    layers::{Layers, Palette},
    volume::{Axis, Volume},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditField {
    Both,
    Minimum,
    Maximum,
    LayerOpacity,
    AtlasOpacity,
    AddPath,
}

pub struct WindowEdit {
    pub field: EditField,
    pub text: String,
    pub error: Option<String>,
    pub completion: Option<PathMatches>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    Help,
    Info,
}

pub struct App {
    pub axis: Axis,
    pub index: usize,
    pub stride: usize,
    pub rotation: u16,
    pub window: (f32, f32),
    pub default_window: (f32, f32),
    pub base_palette: Palette,
    pub base_opacity: f32,
    pub transparent_background: bool,
    pub pixel_budget: usize,
    pub atlas_aa: bool,
    pub highlighted_region: Option<i32>,
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
    pub popup: Option<Popup>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Redraw,
    Ui,
    Layout,
    LoadOverlay(PathBuf),
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
            base_opacity: 1.0,
            transparent_background: false,
            pixel_budget: 1_000_000,
            atlas_aa: false,
            highlighted_region: None,
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
            popup: None,
            notice: None,
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
        if m.contains(KeyModifiers::CONTROL) {
            4
        } else if m.contains(KeyModifiers::SHIFT) || uppercase {
            3
        } else if m.intersects(KeyModifiers::SUPER | KeyModifiers::ALT) {
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

    pub fn selected_opacity(&self, layers: &Layers) -> f32 {
        if self.selected == 0 {
            self.base_opacity
        } else {
            layers.overlays[self.selected - 1].opacity
        }
    }

    fn edit_key(&mut self, layers: &mut Layers, key: KeyCode, modifiers: KeyModifiers) -> Action {
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
                if field == EditField::AddPath {
                    if text.is_empty() {
                        if let Some(e) = self.edit.as_mut() {
                            e.error = Some("Enter an image path".into());
                        }
                        return Action::Ui;
                    }
                    self.edit = None;
                    let path = path_complete::resolve(&text);
                    return Action::LoadOverlay(path);
                }
                if field == EditField::LayerOpacity || field == EditField::AtlasOpacity {
                    let value = text
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v));
                    return if let Some(value) = value {
                        if field == EditField::AtlasOpacity {
                            if let Some(atlas) = &mut layers.atlas {
                                atlas.opacity = value;
                            }
                        } else if self.selected == 0 {
                            self.base_opacity = value;
                        } else {
                            layers.overlays[self.selected - 1].opacity = value;
                        }
                        self.edit = None;
                        Action::Redraw
                    } else {
                        if let Some(e) = self.edit.as_mut() {
                            e.error = Some("Opacity must be 0..1".into());
                        }
                        Action::Ui
                    };
                }
                let result = parse_edit(field, &text, self.selected_window(layers));
                match result {
                    Ok(window) => {
                        self.set_window(layers, window);
                        self.edit = None;
                        Action::Redraw
                    }
                    Err(error) => {
                        if let Some(e) = self.edit.as_mut() {
                            e.error = Some(error);
                        }
                        Action::Ui
                    }
                }
            }
            KeyCode::Tab | KeyCode::BackTab if editor.field == EditField::AddPath => {
                path_complete::advance(
                    &mut editor.text,
                    &mut editor.completion,
                    key == KeyCode::BackTab || modifiers.contains(KeyModifiers::SHIFT),
                );
                editor.error = None;
                Action::Ui
            }
            KeyCode::Backspace => {
                editor.text.pop();
                editor.error = None;
                editor.completion = None;
                Action::Ui
            }
            KeyCode::Char(c)
                if editor.text.len() < 1024
                    && (editor.field == EditField::AddPath
                        || (c.is_ascii_digit()
                            || matches!(c, ',' | '.' | '-' | '+' | 'e' | 'E'))) =>
            {
                editor.text.push(c);
                editor.error = None;
                editor.completion = None;
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
            return self.edit_key(layers, key.code, m);
        }
        if let Some(popup) = self.popup {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Char('/') => {
                    self.popup = None;
                    Action::Layout
                }
                KeyCode::Char('I') if popup == Popup::Info => {
                    self.popup = None;
                    Action::Layout
                }
                _ => Action::None,
            };
        }
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && m.contains(KeyModifiers::CONTROL))
        {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Char('?') | KeyCode::Char('/') => {
                self.popup = Some(Popup::Help);
                Action::Layout
            }
            KeyCode::Char('I') => {
                self.popup = Some(Popup::Info);
                Action::Layout
            }
            KeyCode::Char('n') => {
                self.edit = Some(WindowEdit {
                    field: EditField::AddPath,
                    text: String::new(),
                    error: None,
                    completion: None,
                });
                Action::Ui
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                // Kitty enhanced keys can report uppercase O as lowercase 'o' + SHIFT.
                let field = if key.code == KeyCode::Char('O') || m.contains(KeyModifiers::SHIFT) {
                    EditField::AtlasOpacity
                } else {
                    EditField::LayerOpacity
                };
                if field == EditField::AtlasOpacity && layers.atlas.is_none() {
                    self.notice = Some("No atlas loaded".into());
                    return Action::Ui;
                }
                self.edit = Some(WindowEdit {
                    field,
                    text: String::new(),
                    error: None,
                    completion: None,
                });
                Action::Ui
            }
            KeyCode::Char(';') => {
                self.selected = (self.selected + 1) % (layers.overlays.len() + 1);
                Action::Ui
            }
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
                // Crossterm enhanced-key reporting may send Shift+m as
                // Char('m') + SHIFT instead of Char('M').
                let field = match key.code {
                    KeyCode::Char('M') => EditField::Maximum,
                    KeyCode::Char('m') if m.contains(KeyModifiers::SHIFT) => EditField::Maximum,
                    KeyCode::Char('m') => EditField::Minimum,
                    _ => EditField::Both,
                };
                self.edit = Some(WindowEdit {
                    field,
                    text: String::new(),
                    error: None,
                    completion: None,
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
            KeyCode::Char('g') => {
                self.atlas_aa = !self.atlas_aa;
                Action::Redraw
            }
            KeyCode::Char('b') => {
                self.transparent_background = !self.transparent_background;
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
        _ => return Err("Not an intensity range".into()),
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
            datatype: 16,
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
        assert_eq!(app.index, 48); // shift -3
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Right,
            KeyModifiers::CONTROL,
        );
        assert_eq!(app.index, 52); // control +4
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Left,
            KeyModifiers::ALT,
        );
        assert_eq!(app.index, 50); // alt -2
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Right,
            KeyModifiers::SUPER,
        );
        assert_eq!(app.index, 52); // Command +2 when forwarded
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
            KeyCode::Char('b'),
            KeyModifiers::NONE,
        );
        assert!(app.transparent_background);
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
    fn help_and_opacity_editing() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 2, 1, 0, (0., 100.), "base".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.popup, Some(Popup::Help));
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Esc,
            KeyModifiers::NONE,
        );
        assert_eq!(app.popup, None);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('o'),
            KeyModifiers::NONE,
        );
        for c in "0.4".chars() {
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char(c),
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
        assert!((app.base_opacity - 0.4).abs() < 1e-6);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('n'),
            KeyModifiers::NONE,
        );
        for c in "~/test.nii.gz".chars() {
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char(c),
                KeyModifiers::NONE,
            );
        }
        assert!(matches!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Enter,
                KeyModifiers::NONE
            ),
            Action::LoadOverlay(_)
        ));
    }
    #[test]
    fn shifted_m_edits_maximum_even_with_lowercase_keycode() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 10, 1, 0, (0., 100.), "test".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('m'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::Maximum);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('5'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('0'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        assert_eq!(app.window, (0., 50.));
    }
    #[test]
    fn shifted_o_edits_atlas_opacity_including_zero() {
        use crate::layers::Atlas;
        use ndarray::Array3;
        use std::collections::HashMap;
        let vol = example();
        let labels = Volume {
            data: Array3::zeros((3, 4, 5)),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let mut layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: labels,
                wireframe: false,
                labels: None,
                fallback: [255, 255, 255],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 100.), "base".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('o'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::AtlasOpacity);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('0'),
            KeyModifiers::NONE,
        );
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Enter,
                KeyModifiers::NONE
            ),
            Action::Redraw
        );
        assert_eq!(layers.atlas.as_ref().unwrap().opacity, 0.0);
        assert_eq!(app.base_opacity, 1.0);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('O'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::AtlasOpacity);
    }
    #[test]
    fn aa_toggle_is_off_by_default() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 100.), "base".into());
        assert!(!app.atlas_aa);
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('g'),
                KeyModifiers::NONE
            ),
            Action::Redraw
        );
        assert!(app.atlas_aa);
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
