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
    SaveScene,
    MontageStart,
    MontageStop,
    MontageColumns,
    MontageRows,
    MontageGap,
    MontageInterval,
    AtlasBlur,
}

#[derive(Clone)]
pub struct WindowEdit {
    pub field: EditField,
    pub text: String,
    pub error: Option<String>,
    pub completion: Option<PathMatches>,
    pub checked: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    Help,
    MontageHelp,
    Info,
}

/// A montage shows the inclusive slice range in a single image, divided into
/// pages at the chosen number of columns. The atlas is rendered above every tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MontageState {
    pub enabled: bool,
    pub start: usize,
    pub stop: usize,
    /// Difference in slice indices between consecutive thumbnails.
    pub interval: usize,
    /// Fewer columns means larger (zoomed-in) thumbnails.
    pub columns: usize,
    /// Gap in output raster pixels (at the selected pixel budget).
    pub gap: usize,
    pub rows: usize,
    pub show_numbers: bool,
    pub show_clusters: bool,
}

impl Default for MontageState {
    fn default() -> Self {
        Self {
            enabled: false,
            start: 0,
            stop: 0,
            interval: 0,
            columns: 4,
            gap: 8,
            rows: 3,
            show_numbers: false,
            show_clusters: false,
        }
    }
}

impl MontageState {
    pub fn count(self) -> usize {
        let span = self.stop.saturating_sub(self.start);
        if self.interval == 0 {
            (span + 1).min(12)
        } else {
            span / self.interval + 1
        }
    }
    pub fn slice_at(self, n: usize) -> usize {
        let n = n.min(self.count() - 1);
        if self.interval != 0 {
            self.start + n * self.interval
        } else if self.count() == 1 {
            self.start
        } else {
            self.start
                + ((self.stop - self.start) * n + (self.count() - 1) / 2) / (self.count() - 1)
        }
    }
    pub fn position(self, slice: usize) -> usize {
        let d = slice.clamp(self.start, self.stop) - self.start;
        if self.interval != 0 {
            ((d + self.interval / 2) / self.interval).min(self.count() - 1)
        } else if self.stop == self.start {
            0
        } else {
            (d * (self.count() - 1) + (self.stop - self.start) / 2) / (self.stop - self.start)
        }
    }
    pub fn spacing(self) -> usize {
        if self.interval > 0 {
            self.interval
        } else {
            ((self.stop - self.start) / self.count().saturating_sub(1).max(1)).max(1)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMode {
    Slice,
    Max,
    Mean,
    MinPositive,
}

impl ProjectionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Slice => "slice",
            Self::Max => "max projection",
            Self::Mean => "average projection",
            Self::MinPositive => "minimum positive projection",
        }
    }
}

#[derive(Clone)]
pub struct App {
    pub montage: MontageState,
    pub montage_preset_pending: bool,
    // Session-only slice position before entering montage.
    pub pre_montage_slice: Option<(Axis, usize)>,
    pub atlas_blur: f32,
    pub projection: ProjectionMode,
    /// 0 = major divisions, 1 = broad, 2 = silhouette. Visibility is independent.
    pub projected_atlas_detail: u8,
    pub show_projected_atlas: bool,
    // Explicit choices keyed by stable image ID, not stack position.
    pub layer_projections: Vec<(usize, ProjectionMode)>,
    pub projection_cache: std::rc::Rc<crate::projection::ProjectionCache>,
    pub axis: Axis,
    pub index: usize,
    pub stride: usize,
    pub rotation: u16,
    // Axis-specific screen transforms, saved when leaving an axis.
    pub axis_transforms: [(u16, bool, bool); 3],
    pub cut_layer: Option<usize>,
    pub window: (f32, f32),
    pub default_window: (f32, f32),
    pub base_palette: Palette,
    pub base_opacity: f32,
    pub base_visible: bool,
    pub transparent_background: bool,
    pub pixel_budget: usize,
    pub atlas_aa: bool,
    pub highlighted_region: Option<i32>,
    pub path: String,
    pub atlas_name: Option<String>,
    /// Position within the compositing stack (0 = bottom, highest = top).
    pub selected: usize,
    /// Image IDs from bottom to top: 0 = original base; 1+ = loaded overlays.
    /// The atlas is always composited above this stack and is never reordered.
    pub layer_order: Vec<usize>,
    pub flip_h: bool,
    pub flip_v: bool,
    pub show_atlas: bool,
    pub show_overlays: bool,
    pub fullscreen: bool,
    pub show_layer_list: bool,
    pub inspect_mode: bool,
    pub probe: Option<(usize, usize)>,
    pub edit: Option<WindowEdit>,
    /// Previous file filter while the picker is accepting a zoxide directory query.
    pub picker_jump_from: Option<String>,
    pub popup: Option<Popup>,
    pub popup_scroll: usize,
    pub notice: Option<String>,
    /// Most recently loaded or saved scene; shown as the default save path.
    pub scene_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Redraw,
    Ui,
    Layout,
    LoadOverlays(Vec<PathBuf>),
    SaveScene(PathBuf),
    CopyRegion,
    YankLayer,
    PasteLayer(bool),
    UnloadLayer,
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
            montage: MontageState::default(),
            montage_preset_pending: true,
            pre_montage_slice: None,
            atlas_blur: 0.,
            projection: ProjectionMode::Slice,
            projected_atlas_detail: 0,
            show_projected_atlas: false,
            layer_projections: Vec::new(),
            projection_cache: Default::default(),
            axis,
            index,
            stride,
            rotation,
            axis_transforms: [(rotation, false, false); 3],
            cut_layer: None,
            window,
            default_window: window,
            base_palette: Palette::Gray,
            base_opacity: 1.0,
            base_visible: true,
            transparent_background: false,
            pixel_budget: 1_000_000,
            atlas_aa: false,
            highlighted_region: None,
            path,
            atlas_name: None,
            selected: 0,
            layer_order: vec![0],
            flip_h: false,
            flip_v: false,
            show_atlas: true,
            show_overlays: true,
            fullscreen: false,
            show_layer_list: false,
            inspect_mode: false,
            probe: None,
            edit: None,
            picker_jump_from: None,
            popup: None,
            popup_scroll: 0,
            notice: None,
            scene_path: None,
        }
    }

    /// Called after the reference volume is available, or when the slicing axis
    /// changes. Avoids letting a saved bound exceed the axis depth.
    pub fn reset_montage(&mut self, vol: &Volume) {
        let last = vol.depth(self.axis) - 1;
        self.montage = MontageState {
            enabled: self.montage.enabled,
            start: last / 10,
            stop: last - last / 10,
            ..MontageState::default()
        };
        self.montage_preset_pending = true;
    }
    // Only scan the atlas when montage is entered or the preset is requested.
    // Without a solid atlas, use the central 80% of the reference grid.
    fn montage_preset(&mut self, vol: &Volume, layers: &Layers) {
        self.reset_montage(vol);
        if let Some(solid) = layers.atlas.as_ref().and_then(|a| {
            if a.wireframe {
                a.labels.as_ref()
            } else {
                Some(&a.vol)
            }
        }) {
            let (w, h, _) = solid.plane(self.axis);
            let occupied = |i| {
                (0..h).any(|y| {
                    (0..w).any(|x| crate::layers::label_id(solid.sample(self.axis, i, x, y)) != 0)
                })
            };
            let depth = solid.depth(self.axis);
            if let (Some(first), Some(last)) = (
                (0..depth).find(|&i| occupied(i)),
                (0..depth).rev().find(|&i| occupied(i)),
            ) {
                let margin = (last - first) / 10;
                self.montage.start = first + margin;
                self.montage.stop = last - margin;
            }
        }
        self.montage_preset_pending = false;
        self.index = self.montage.start;
    }
    pub fn normalize_montage(&mut self, vol: &Volume) {
        let last = vol.depth(self.axis) - 1;
        self.montage.start = self.montage.start.min(last);
        self.montage.stop = self.montage.stop.min(last).max(self.montage.start);
        self.montage.interval = self.montage.interval.min(256);
        self.montage.columns = self.montage.columns.clamp(1, 12);
        self.montage.rows = self.montage.rows.clamp(1, 12);
        self.montage.gap = self.montage.gap.min(32);
        if self.montage.enabled {
            self.snap_montage_index();
        }
    }
    fn snap_montage_index(&mut self) {
        self.index = self.montage.slice_at(self.montage.position(self.index));
    }
    pub fn numeric_montage_value(&self, field: EditField) -> Option<f32> {
        Some(match field {
            EditField::MontageStart => self.montage.start as f32,
            EditField::MontageStop => self.montage.stop as f32,
            EditField::MontageColumns => self.montage.columns as f32,
            EditField::MontageRows => self.montage.rows as f32,
            EditField::MontageGap => self.montage.gap as f32,
            EditField::MontageInterval => self.montage.interval as f32,
            EditField::AtlasBlur => self.atlas_blur,
            _ => return None,
        })
    }
    fn open_number(&mut self, field: EditField) -> Action {
        self.edit = Some(WindowEdit {
            field,
            text: String::new(),
            error: None,
            completion: None,
            checked: Vec::new(),
        });
        if self.fullscreen {
            self.fullscreen = false;
            Action::Layout
        } else {
            Action::Ui
        }
    }
    fn apply_number(&mut self, vol: &Volume, field: EditField, value: f32) -> Result<(), String> {
        if field == EditField::AtlasBlur {
            if !value.is_finite() || !(0.0..=4.0).contains(&value) {
                return Err("Blur sigma must be 0..4 px".into());
            }
            self.atlas_blur = value;
            return Ok(());
        }
        if !value.is_finite() || value < 0. || value.fract() != 0. {
            return Err("Enter a nonnegative integer".into());
        }
        let n = value as usize;
        let valid = match field {
            EditField::MontageStart => n <= self.montage.stop,
            EditField::MontageStop => n >= self.montage.start && n < vol.depth(self.axis),
            EditField::MontageColumns | EditField::MontageRows => (1..=12).contains(&n),
            EditField::MontageGap => n <= 32,
            EditField::MontageInterval => n <= 256,
            _ => false,
        };
        if !valid {
            return Err("Outside allowed range (start ≤ stop; within volume; grid 1..12; gap 0..32; interval 0..256)".into());
        }
        match field {
            EditField::MontageStart => {
                self.montage.start = n;
                self.montage_preset_pending = false;
            }
            EditField::MontageStop => {
                self.montage.stop = n;
                self.montage_preset_pending = false;
            }
            EditField::MontageColumns => self.montage.columns = n,
            EditField::MontageRows => self.montage.rows = n,
            EditField::MontageGap => self.montage.gap = n,
            EditField::MontageInterval => self.montage.interval = n,
            _ => unreachable!(),
        }
        self.index = self.montage.start;
        Ok(())
    }

    /// Slice mode bypasses all overrides; unconfigured projected layers use max.
    pub fn projection_for(&self, id: usize) -> ProjectionMode {
        if self.projection == ProjectionMode::Slice {
            return ProjectionMode::Slice;
        }
        self.layer_projections
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, mode)| *mode)
            .unwrap_or(self.projection)
    }

    fn set_projection(&mut self, mode: ProjectionMode) -> Action {
        if self.montage.enabled {
            self.notice =
                Some("Projection views are only available in the single-slice viewer".into());
            return Action::Ui;
        }
        if self.projection == ProjectionMode::Slice {
            self.inspect_mode = false;
            self.probe = None;
            self.highlighted_region = None;
            self.projection = ProjectionMode::Max;
        }
        let id = self.selected_layer_id();
        if let Some((_, old)) = self.layer_projections.iter_mut().find(|(i, _)| *i == id) {
            *old = mode;
        } else {
            self.layer_projections.push((id, mode));
        }
        Action::Redraw
    }

    /// Keep the stack in sync after a new aligned image has been loaded.
    /// Existing order is preserved; each new image is appended on top.
    pub fn sync_layer_order(&mut self, overlay_count: usize) {
        let count = overlay_count + 1;
        if self.layer_order.len() == count {
            return;
        } // common interactive path
        self.layer_order.retain(|&id| id < count);
        for id in 0..count {
            if !self.layer_order.contains(&id) {
                self.layer_order.push(id);
            }
        }
        self.selected = self.selected.min(count - 1);
    }

    /// Return the physical image ID at this visible stack position.
    /// The fallback also supports small unit fixtures made without loading.
    pub fn layer_at(&self, position: usize) -> usize {
        self.layer_order.get(position).copied().unwrap_or(position)
    }

    pub fn layer_number(&self, position: usize) -> usize {
        self.layer_order.len().saturating_sub(position)
    }

    pub fn selected_layer_id(&self) -> usize {
        self.layer_at(self.selected)
    }

    pub fn layer_visible(&self, layers: &Layers, id: usize) -> bool {
        if id == 0 {
            self.base_visible
        } else {
            self.show_overlays && layers.overlays[id - 1].visible
        }
    }

    fn move_selected_layer(&mut self, displacement: isize) -> Action {
        let Some(next) = self.selected.checked_add_signed(displacement) else {
            return Action::None;
        };
        if next >= self.layer_order.len() {
            return Action::None;
        }
        self.layer_order.swap(self.selected, next);
        self.selected = next; // Follow the image, not its previous stack position.
        Action::Redraw
    }

    pub fn selected_window(&self, layers: &Layers) -> (f32, f32) {
        let id = self.selected_layer_id();
        if id == 0 {
            self.window
        } else {
            layers.overlays[id - 1].window
        }
    }

    fn set_window(&mut self, layers: &mut Layers, window: (f32, f32)) {
        let id = self.selected_layer_id();
        if id == 0 {
            self.window = window;
        } else {
            layers.overlays[id - 1].window = window;
        }
    }

    fn default_selected_window(&self, layers: &Layers) -> (f32, f32) {
        let id = self.selected_layer_id();
        if id == 0 {
            self.default_window
        } else {
            layers.overlays[id - 1].default_window
        }
    }

    pub fn selected_palette(&self, layers: &Layers) -> Palette {
        let id = self.selected_layer_id();
        if id == 0 {
            self.base_palette
        } else {
            layers.overlays[id - 1].palette
        }
    }

    fn cycle_palette(&mut self, layers: &mut Layers) {
        let id = self.selected_layer_id();
        if id == 0 {
            self.base_palette = self.base_palette.next();
        } else {
            let layer = &mut layers.overlays[id - 1];
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
            self.flip_h = !self.flip_h;
            self.probe = None;
            return;
        }
        let old_len = vol.depth(self.axis);
        let new_len = vol.depth(axis);
        let fraction = if old_len > 1 {
            self.index as f64 / (old_len - 1) as f64
        } else {
            0.5
        };
        self.axis_transforms[self.axis.index()] = (self.rotation, self.flip_h, self.flip_v);
        self.axis = axis;
        (self.rotation, self.flip_h, self.flip_v) = self.axis_transforms[axis.index()];
        self.probe = None;
        self.index = (fraction * (new_len - 1) as f64).round() as usize;
        self.reset_montage(vol);
        if self.montage.enabled {
            self.snap_montage_index();
        }
    }

    pub fn move_slices(&mut self, vol: &Volume, delta: isize) -> bool {
        if self.projection != ProjectionMode::Slice {
            return false;
        }
        let new_index = if self.montage.enabled {
            let m = self.montage;
            m.slice_at(m.position(self.index).saturating_add_signed(delta))
        } else {
            self.index
                .saturating_add_signed(delta)
                .min(vol.depth(self.axis) - 1)
        };
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
        let id = self.selected_layer_id();
        if id == 0 {
            self.base_opacity
        } else {
            layers.overlays[id - 1].opacity
        }
    }

    fn picker_key(&mut self, key: KeyCode, modifiers: KeyModifiers) -> Action {
        let editor = self.edit.as_mut().expect("picker is open");
        let jump_key = key == KeyCode::Char('\x1a')
            || (matches!(key, KeyCode::Char('z' | 'Z'))
                && modifiers.contains(KeyModifiers::CONTROL));
        if self.picker_jump_from.is_some() {
            if jump_key || key == KeyCode::Esc {
                editor.text = self.picker_jump_from.take().unwrap();
                editor.completion = Some(path_complete::search(&editor.text));
                editor.error = None;
                return Action::Layout;
            }
            match key {
                KeyCode::Enter => match path_complete::zoxide_directory(&editor.text) {
                    Ok(dir) => {
                        self.picker_jump_from = None;
                        editor.text = dir;
                        editor.completion = Some(path_complete::search(&editor.text));
                        editor.error = None;
                    }
                    Err(e) => editor.error = Some(e),
                },
                KeyCode::Backspace => {
                    editor.text.pop();
                    editor.error = None;
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !modifiers.intersects(
                            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                        )
                        && editor.text.len() < 4096 =>
                {
                    editor
                        .text
                        .push(if c == '-' && modifiers.contains(KeyModifiers::SHIFT) {
                            '_'
                        } else {
                            c
                        });
                    editor.error = None;
                }
                _ => return Action::None,
            }
            return Action::Layout;
        }
        if jump_key {
            self.picker_jump_from = Some(std::mem::take(&mut editor.text));
            editor.error = None;
            return Action::Layout;
        }
        match key {
            KeyCode::Esc => {
                self.edit = None;
                Action::Layout
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                if let Some(matches) = editor.completion.as_mut() {
                    let delta = match key {
                        KeyCode::Up => -1,
                        KeyCode::Down => 1,
                        KeyCode::PageUp => -10,
                        _ => 10,
                    };
                    matches.move_by(delta);
                }
                Action::Layout
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(matches) = editor.completion.as_mut() {
                    path_complete::advance(
                        &mut editor.text,
                        matches,
                        key == KeyCode::BackTab || modifiers.contains(KeyModifiers::SHIFT),
                    );
                }
                Action::Layout
            }
            KeyCode::Right => {
                if let Some(m) = editor.completion.as_ref().and_then(|m| m.selected()) {
                    if m.is_dir {
                        editor.text = m.path.clone();
                        editor.completion = Some(path_complete::search(&editor.text));
                        editor.error = None;
                    } else {
                        editor.text = m.path.clone();
                        editor.completion = Some(path_complete::search(&editor.text));
                    }
                }
                Action::Layout
            }
            KeyCode::Char(' ') => {
                if let Some(m) = editor.completion.as_ref().and_then(|m| m.selected()) {
                    if !m.is_dir {
                        let path = path_complete::resolve(&m.path);
                        if let Some(i) = editor.checked.iter().position(|p| p == &path) {
                            editor.checked.remove(i);
                        } else {
                            editor.checked.push(path);
                        }
                    }
                }
                Action::Layout
            }
            KeyCode::Enter => {
                if !editor.checked.is_empty() {
                    let paths = editor.checked.clone();
                    return Action::LoadOverlays(paths);
                }
                let candidate = editor.completion.as_ref().and_then(|m| m.selected());
                let path = candidate
                    .map(|m| m.path.clone())
                    .unwrap_or_else(|| editor.text.clone());
                if path.is_empty() {
                    editor.error = Some("Choose an image or type a filename substring".into());
                    return Action::Layout;
                }
                let path = path_complete::resolve(&path);
                if path.is_dir() {
                    let mut dir = path.to_string_lossy().into_owned();
                    if !dir.ends_with('/') {
                        dir.push('/');
                    }
                    editor.text = dir;
                    editor.completion = Some(path_complete::search(&editor.text));
                    editor.error = None;
                    return Action::Layout;
                }
                if !path_complete::is_nifti_file(&path) {
                    editor.error = Some(format!(
                        "Not an existing .nii or .nii.gz file: {}",
                        path.display()
                    ));
                    return Action::Layout;
                }
                editor.checked.push(path.clone());
                Action::LoadOverlays(vec![path])
            }
            KeyCode::Backspace => {
                editor.text.pop();
                editor.completion = Some(path_complete::search(&editor.text));
                editor.error = None;
                Action::Layout
            }
            KeyCode::Char(c) if editor.text.len() < 4096 && !c.is_control() => {
                // Ghostty/Rex may report Shift+- as Char('-') with SHIFT, not Char('_').
                // Accept both forms; keep the unshifted hyphen unchanged.
                let typed = if c == '-' && modifiers.contains(KeyModifiers::SHIFT) {
                    '_'
                } else {
                    c
                };
                editor.text.push(typed);
                editor.completion = Some(path_complete::search(&editor.text));
                editor.error = None;
                Action::Layout
            }
            _ => Action::None,
        }
    }

    fn edit_key(
        &mut self,
        vol: &Volume,
        layers: &mut Layers,
        key: KeyCode,
        modifiers: KeyModifiers,
    ) -> Action {
        if self
            .edit
            .as_ref()
            .is_some_and(|e| e.field == EditField::AddPath)
        {
            return self.picker_key(key, modifiers);
        }
        if let Some((field, current)) = self
            .edit
            .as_ref()
            .and_then(|e| self.numeric_montage_value(e.field).map(|v| (e.field, v)))
        {
            let text = self.edit.as_ref().unwrap().text.trim().to_owned();
            match key {
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                    let step = if field == EditField::AtlasBlur {
                        0.25
                    } else {
                        1.0
                    };
                    let sign = if matches!(key, KeyCode::Up | KeyCode::Right) {
                        1.0
                    } else {
                        -1.0
                    };
                    let draft = text
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .unwrap_or(current);
                    let (low, high) = match field {
                        EditField::MontageStart => (0., self.montage.stop as f32),
                        EditField::MontageStop => {
                            (self.montage.start as f32, (vol.depth(self.axis) - 1) as f32)
                        }
                        EditField::MontageColumns | EditField::MontageRows => (1., 12.),
                        EditField::MontageGap => (0., 32.),
                        EditField::MontageInterval => (0., 256.),
                        _ => (0., 4.),
                    };
                    let value = (draft + sign * step * Self::multiplier(modifiers, false) as f32)
                        .clamp(low, high);
                    let editor = self.edit.as_mut().unwrap();
                    editor.text = value.to_string();
                    editor.error = None;
                    return Action::Ui;
                }
                KeyCode::Enter => {
                    let result = if text.is_empty() {
                        Ok(())
                    } else {
                        text.parse::<f32>()
                            .map_err(|_| "Enter a number".to_owned())
                            .and_then(|v| self.apply_number(vol, field, v))
                    };
                    return match result {
                        Ok(()) => {
                            self.edit = None;
                            Action::Layout
                        }
                        Err(e) => {
                            self.edit.as_mut().unwrap().error = Some(e);
                            Action::Ui
                        }
                    };
                }
                KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                    let e = self.edit.as_mut().unwrap();
                    e.text.clear();
                    e.error = None;
                    return Action::Ui;
                }
                _ => {}
            }
        }
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
                if field == EditField::SaveScene {
                    if text.is_empty() {
                        if let Some(e) = self.edit.as_mut() {
                            e.error = Some("Enter a scene filename".into());
                        }
                        return Action::Ui;
                    }
                    self.edit = None;
                    return Action::SaveScene(path_complete::resolve(&text));
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
                        } else {
                            let id = self.selected_layer_id();
                            if id == 0 {
                                self.base_opacity = value;
                            } else {
                                layers.overlays[id - 1].opacity = value;
                            }
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
            KeyCode::Backspace => {
                editor.text.pop();
                editor.error = None;
                Action::Ui
            }
            KeyCode::Char(c)
                if editor.field == EditField::SaveScene
                    && editor.text.len() < 4096
                    && !c.is_control() =>
            {
                let typed = if c == '-' && modifiers.contains(KeyModifiers::SHIFT) {
                    '_'
                } else {
                    c
                };
                editor.text.push(typed);
                editor.error = None;
                Action::Ui
            }
            KeyCode::Char(c)
                if editor.text.len() < 1024
                    && (c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | '+' | 'e' | 'E')) =>
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
        // Global emergency exit must precede all modal input handlers.
        if m.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C' | '\x03'))
        {
            return Action::Quit;
        }
        if key.code == KeyCode::Char('\x03') {
            return Action::Quit;
        }
        // Holding q must not close a menu and then quit on a key-repeat event.
        if key.kind == KeyEventKind::Repeat
            && key.code == KeyCode::Char('q')
            && !self
                .edit
                .as_ref()
                .is_some_and(|e| matches!(e.field, EditField::AddPath | EditField::SaveScene))
        {
            return Action::None;
        }
        // Destructive or duplicating commands should not auto-repeat outside text entry.
        if key.kind == KeyEventKind::Repeat
            && self.edit.is_none()
            && self.popup.is_none()
            && (key.code == KeyCode::Char('d') && !m.contains(KeyModifiers::SHIFT)
                || self.show_layer_list && matches!(key.code, KeyCode::Char('p' | 'P' | 'X' | 'Y')))
        {
            return Action::None;
        }
        self.sync_layer_order(layers.overlays.len());
        if let Some(editor) = &self.edit {
            // Filename fields accept literal q. Numeric fields do not need it.
            if key.code == KeyCode::Char('q')
                && !matches!(editor.field, EditField::AddPath | EditField::SaveScene)
            {
                self.edit = None;
                return Action::Layout;
            }
            return self.edit_key(vol, layers, key.code, m);
        }
        if let Some(popup) = self.popup {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Char('/') => {
                    self.popup = None;
                    self.popup_scroll = 0;
                    Action::Layout
                }
                KeyCode::Up => {
                    self.popup_scroll = self.popup_scroll.saturating_sub(1);
                    Action::Layout
                }
                KeyCode::Down => {
                    self.popup_scroll = self.popup_scroll.saturating_add(1);
                    Action::Layout
                }
                KeyCode::PageUp => {
                    self.popup_scroll = self.popup_scroll.saturating_sub(10);
                    Action::Layout
                }
                KeyCode::PageDown => {
                    self.popup_scroll = self.popup_scroll.saturating_add(10);
                    Action::Layout
                }
                KeyCode::Char('i') if popup == Popup::Info && !m.contains(KeyModifiers::SHIFT) => {
                    self.popup = None;
                    Action::Layout
                }
                _ => Action::None,
            };
        }
        // The layer list is top-first, whereas selected is a bottom-based
        // stack position. Modal editors/popups take precedence over this list.
        if self.show_layer_list && !self.fullscreen {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.show_layer_list = false;
                    self.cut_layer = None;
                    return Action::Layout;
                }
                KeyCode::Char('X') | KeyCode::Char('x')
                    if key.code == KeyCode::Char('X') || m.contains(KeyModifiers::SHIFT) =>
                {
                    self.cut_layer = Some(self.selected_layer_id());
                    return Action::Ui;
                }
                KeyCode::Char('p' | 'P') => {
                    let above = key.code == KeyCode::Char('P') || m.contains(KeyModifiers::SHIFT);
                    if let Some(id) = self.cut_layer.take() {
                        let target = self.selected_layer_id();
                        if id == target {
                            return Action::Ui;
                        }
                        if let Some(old) = self.layer_order.iter().position(|&i| i == id) {
                            self.layer_order.remove(old);
                            let pos = self.layer_order.iter().position(|&i| i == target).unwrap()
                                + usize::from(above);
                            self.layer_order.insert(pos, id);
                            self.selected = pos;
                            return Action::Redraw;
                        }
                        return Action::Ui;
                    }
                    return Action::PasteLayer(above);
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                    let next = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) {
                        self.selected
                            .saturating_add(1)
                            .min(self.layer_order.len() - 1)
                    } else {
                        self.selected.saturating_sub(1)
                    };
                    if next == self.selected {
                        return Action::None;
                    }
                    self.selected = next;
                    // Cluster labels in a montage can depend on the selection.
                    return if self.montage.enabled && self.montage.show_clusters {
                        Action::Redraw
                    } else {
                        Action::Ui
                    };
                }
                _ => {}
            }
        }
        if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Char('?') | KeyCode::Char('/') => {
                self.popup = Some(if self.montage.enabled {
                    Popup::MontageHelp
                } else {
                    Popup::Help
                });
                self.popup_scroll = 0;
                Action::Layout
            }
            KeyCode::Char('i') if !m.contains(KeyModifiers::SHIFT) => {
                self.popup = Some(Popup::Info);
                self.popup_scroll = 0;
                Action::Layout
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                let filename = self
                    .scene_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("{}.niv.toml", self.path));
                self.edit = Some(WindowEdit {
                    field: EditField::SaveScene,
                    text: filename,
                    error: None,
                    completion: None,
                    checked: Vec::new(),
                });
                Action::Ui
            }
            KeyCode::Char('n') if self.montage.enabled && m.contains(KeyModifiers::SHIFT) => {
                self.montage.show_numbers = !self.montage.show_numbers;
                Action::Redraw
            }
            KeyCode::Char('n') => {
                self.picker_jump_from = None;
                self.edit = Some(WindowEdit {
                    field: EditField::AddPath,
                    text: String::new(),
                    error: None,
                    completion: Some(path_complete::search("")),
                    checked: Vec::new(),
                });
                Action::Layout
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
                    checked: Vec::new(),
                });
                Action::Ui
            }
            KeyCode::Char(';') => {
                self.selected = (self.selected + 1) % self.layer_order.len();
                if self.montage.enabled && self.montage.show_clusters {
                    Action::Redraw
                } else {
                    Action::Ui
                }
            }
            KeyCode::Tab => {
                self.fullscreen = !self.fullscreen;
                if self.fullscreen {
                    self.show_layer_list = false;
                    self.cut_layer = None;
                }
                Action::Layout
            }
            KeyCode::Char('T') | KeyCode::Char('t')
                if self.projection != ProjectionMode::Slice
                    && (key.code == KeyCode::Char('T') || m.contains(KeyModifiers::SHIFT)) =>
            {
                if self.show_projected_atlas {
                    self.projected_atlas_detail = (self.projected_atlas_detail + 1) % 3;
                } else {
                    self.show_projected_atlas = true;
                }
                Action::Redraw
            }
            KeyCode::Char('t') => {
                self.show_layer_list = !self.show_layer_list;
                if self.show_layer_list {
                    self.fullscreen = false;
                } else {
                    self.cut_layer = None;
                }
                Action::Layout
            }
            KeyCode::Char(c @ '1'..='9') => {
                let number = (c as u8 - b'0') as usize;
                if number <= self.layer_order.len() {
                    self.selected = self.layer_order.len() - number;
                    if self.montage.enabled && self.montage.show_clusters {
                        Action::Redraw
                    } else {
                        Action::Ui
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Char('B') => self.open_number(EditField::MontageStart),
            KeyCode::Char('b') if m.contains(KeyModifiers::SHIFT) => {
                self.open_number(EditField::MontageStart)
            }
            KeyCode::Char('E') => self.open_number(EditField::MontageStop),
            KeyCode::Char('e') if m.contains(KeyModifiers::SHIFT) => {
                self.open_number(EditField::MontageStop)
            }
            KeyCode::Char('P') => {
                self.projection = ProjectionMode::Slice;
                self.montage_preset(vol, layers);
                Action::Layout
            }
            KeyCode::Char('p') if m.contains(KeyModifiers::SHIFT) => {
                self.projection = ProjectionMode::Slice;
                self.montage_preset(vol, layers);
                Action::Layout
            }
            KeyCode::Char('Z' | 'F' | 'D' | 'U' | 'N' | 'C') if self.montage.enabled => {
                match key.code {
                    KeyCode::Char('Z') => self.open_number(EditField::MontageColumns),
                    KeyCode::Char('F') => self.open_number(EditField::MontageRows),
                    KeyCode::Char('D') => self.open_number(EditField::MontageGap),
                    KeyCode::Char('U') => self.open_number(EditField::MontageInterval),
                    KeyCode::Char('N') => {
                        self.montage.show_numbers = !self.montage.show_numbers;
                        Action::Redraw
                    }
                    _ => {
                        self.montage.show_clusters = !self.montage.show_clusters;
                        Action::Redraw
                    }
                }
            }
            KeyCode::Char('z' | 'f' | 'd' | 'u' | 'c')
                if self.montage.enabled && m.contains(KeyModifiers::SHIFT) =>
            {
                match key.code {
                    KeyCode::Char('z') => self.open_number(EditField::MontageColumns),
                    KeyCode::Char('f') => self.open_number(EditField::MontageRows),
                    KeyCode::Char('d') => self.open_number(EditField::MontageGap),
                    KeyCode::Char('u') => self.open_number(EditField::MontageInterval),
                    _ => {
                        self.montage.show_clusters = !self.montage.show_clusters;
                        Action::Redraw
                    }
                }
            }
            KeyCode::Char('G') => self.open_number(EditField::AtlasBlur),
            KeyCode::Char('g') if m.contains(KeyModifiers::SHIFT) => {
                self.open_number(EditField::AtlasBlur)
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
                    checked: Vec::new(),
                });
                Action::Ui
            }
            KeyCode::Char('p') => {
                self.montage.enabled = !self.montage.enabled;
                if self.montage.enabled {
                    self.pre_montage_slice = Some((self.axis, self.index));
                    self.projection = ProjectionMode::Slice;
                    self.inspect_mode = false;
                    if self.montage_preset_pending {
                        self.montage_preset(vol, layers);
                    }
                    self.index = self.montage.start;
                } else {
                    let depth = vol.depth(self.axis);
                    self.index = match self.pre_montage_slice.take() {
                        Some((axis, index)) if axis == self.axis => index.min(depth - 1),
                        _ => depth / 2,
                    };
                    self.probe = None;
                    self.highlighted_region = None;
                }
                self.normalize_montage(vol);
                Action::Layout
            }
            KeyCode::Char('X') => self.set_projection(ProjectionMode::Max),
            KeyCode::Char('x') if m.contains(KeyModifiers::SHIFT) => {
                self.set_projection(ProjectionMode::Max)
            }
            KeyCode::Char('A') => self.set_projection(ProjectionMode::Mean),
            KeyCode::Char('a') if m.contains(KeyModifiers::SHIFT) => {
                self.set_projection(ProjectionMode::Mean)
            }
            KeyCode::Char('V') => {
                if self.projection == ProjectionMode::Slice {
                    Action::None
                } else {
                    self.projection = ProjectionMode::Slice;
                    self.highlighted_region = None;
                    Action::Redraw
                }
            }
            KeyCode::Char('v') if m.contains(KeyModifiers::SHIFT) => {
                if self.projection == ProjectionMode::Slice {
                    Action::None
                } else {
                    self.projection = ProjectionMode::Slice;
                    self.highlighted_region = None;
                    Action::Redraw
                }
            }
            KeyCode::Char('l') => {
                self.cycle_palette(layers);
                Action::Redraw
            }
            KeyCode::Char('w') => self.move_selected_layer(1),
            KeyCode::Char('d') => {
                if self.layer_order.len() > 1 {
                    Action::UnloadLayer
                } else {
                    self.notice = Some("Keep at least one reference image; Space hides it".into());
                    Action::Ui
                }
            }
            KeyCode::Char('Y') => {
                self.cut_layer = None;
                Action::YankLayer
            }
            KeyCode::Char('y') if m.contains(KeyModifiers::SHIFT) => {
                self.cut_layer = None;
                Action::YankLayer
            }
            KeyCode::Char('U') | KeyCode::Char('u')
                if !self.montage.enabled
                    && (key.code == KeyCode::Char('U') || m.contains(KeyModifiers::SHIFT)) =>
            {
                self.set_projection(ProjectionMode::MinPositive)
            }
            KeyCode::Char('u')
                if !self.montage.enabled && self.projection == ProjectionMode::Slice =>
            {
                Action::CopyRegion
            }
            KeyCode::Char(' ') => {
                let id = self.selected_layer_id();
                if id == 0 {
                    self.base_visible = !self.base_visible;
                } else {
                    let layer = &mut layers.overlays[id - 1];
                    layer.visible = !layer.visible;
                }
                Action::Redraw
            }
            KeyCode::Char('a') => {
                if self.projection == ProjectionMode::Slice {
                    self.show_atlas = !self.show_atlas;
                } else {
                    self.show_projected_atlas = !self.show_projected_atlas;
                }
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
                if self.montage.enabled && self.montage_preset_pending {
                    self.montage_preset(vol, layers);
                }
                Action::Redraw
            }
            KeyCode::Char('y' | 'c') => {
                self.set_axis(vol, Axis::Y);
                if self.montage.enabled && self.montage_preset_pending {
                    self.montage_preset(vol, layers);
                }
                Action::Redraw
            }
            KeyCode::Char('z') => {
                self.set_axis(vol, Axis::Z);
                if self.montage.enabled && self.montage_preset_pending {
                    self.montage_preset(vol, layers);
                }
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
            KeyCode::Char('I' | 'i')
                if self.montage.enabled || self.projection != ProjectionMode::Slice =>
            {
                Action::None
            }
            KeyCode::Char('I' | 'i') => {
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
            KeyCode::Char('+') | KeyCode::Char('=') if self.montage.enabled => {
                self.montage.columns = self.montage.columns.saturating_sub(1).max(1);
                self.montage.rows = self.montage.rows.saturating_sub(1).max(1);
                Action::Layout
            }
            KeyCode::Char('-') | KeyCode::Char('_') if self.montage.enabled => {
                self.montage.columns = (self.montage.columns + 1).min(12);
                self.montage.rows = (self.montage.rows + 1).min(12);
                Action::Layout
            }
            KeyCode::Char('<') if self.montage.enabled => {
                self.montage.gap = self.montage.gap.saturating_sub(1);
                Action::Redraw
            }
            KeyCode::Char('>') if self.montage.enabled => {
                self.montage.gap = (self.montage.gap + 1).min(32);
                Action::Redraw
            }
            KeyCode::Char(',') if self.montage.enabled && m.contains(KeyModifiers::SHIFT) => {
                self.montage.gap = self.montage.gap.saturating_sub(1);
                Action::Redraw
            }
            KeyCode::Char('.') if self.montage.enabled && m.contains(KeyModifiers::SHIFT) => {
                self.montage.gap = (self.montage.gap + 1).min(32);
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
            // Braces (Shift+[ / Shift+]) move the *selected image* through
            // the compositing stack. Support both ordinary keycodes and
            // enhanced-key reporting of '['/']' with SHIFT.
            KeyCode::Char('{') => self.move_selected_layer(-1),
            KeyCode::Char('}') => self.move_selected_layer(1),
            KeyCode::Char('[') if m.contains(KeyModifiers::SHIFT) => self.move_selected_layer(-1),
            KeyCode::Char(']') if m.contains(KeyModifiers::SHIFT) => self.move_selected_layer(1),
            KeyCode::Char('[') if self.montage.enabled => {
                self.montage.interval = self.montage.spacing().saturating_sub(1).max(1);
                self.snap_montage_index();
                Action::Redraw
            }
            KeyCode::Char(']') if self.montage.enabled => {
                self.montage.interval = self.montage.spacing().saturating_add(1).min(256);
                self.snap_montage_index();
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
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End
                if self.projection != ProjectionMode::Slice =>
            {
                Action::None
            }
            KeyCode::PageUp => {
                self.move_slices(
                    vol,
                    if self.montage.enabled {
                        -((self.montage.columns * self.montage.rows) as isize)
                    } else {
                        25
                    },
                );
                Action::Redraw
            }
            KeyCode::PageDown => {
                self.move_slices(
                    vol,
                    if self.montage.enabled {
                        (self.montage.columns * self.montage.rows) as isize
                    } else {
                        -25
                    },
                );
                Action::Redraw
            }
            KeyCode::Home => {
                self.index = if self.montage.enabled {
                    self.montage.start
                } else {
                    0
                };
                Action::Redraw
            }
            KeyCode::End => {
                self.index = if self.montage.enabled {
                    self.montage.slice_at(self.montage.count() - 1)
                } else {
                    vol.depth(self.axis) - 1
                };
                Action::Redraw
            }
            KeyCode::Up
            | KeyCode::Right
            | KeyCode::Left
            | KeyCode::Down
            | KeyCode::Char('h' | 'j' | 'k' | 'H' | 'J' | 'K') => {
                let multiplier =
                    Self::multiplier(m, matches!(key.code, KeyCode::Char('H' | 'J' | 'K')));
                if self.inspect_mode {
                    let (dx, dy) = match key.code {
                        KeyCode::Right => (1, 0),
                        KeyCode::Left | KeyCode::Char('h' | 'H') => (-1, 0),
                        KeyCode::Up | KeyCode::Char('k' | 'K') => (0, -1),
                        _ => (0, 1),
                    };
                    self.move_probe(vol, dx * multiplier as isize, dy * multiplier as isize);
                } else if self.montage.enabled {
                    let delta = match key.code {
                        KeyCode::Up | KeyCode::Char('k' | 'K') => -(self.montage.columns as isize),
                        KeyCode::Down | KeyCode::Char('j' | 'J') => self.montage.columns as isize,
                        KeyCode::Left | KeyCode::Char('h' | 'H') => -1,
                        _ => 1,
                    };
                    self.move_slices(vol, delta * multiplier as isize);
                } else {
                    let sign = match key.code {
                        KeyCode::Up | KeyCode::Right | KeyCode::Char('k' | 'K') => 1,
                        _ => -1,
                    };
                    if !self.move_slices(vol, sign * (self.stride * multiplier) as isize)
                        && self.projection != ProjectionMode::Slice
                    {
                        return Action::None;
                    }
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
            KeyCode::Char('I'),
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
                path: PathBuf::from("overlay.nii.gz"),
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
            KeyCode::Char('1'),
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
            KeyCode::Char('l'),
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
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::AddPath);
        assert!(app.edit.as_ref().unwrap().completion.is_some());
    }
    #[test]
    fn lowercase_i_opens_and_closes_info_but_shifted_i_inspects() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 10, 1, 0, (0., 100.), "test".into());
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('i'),
                KeyModifiers::NONE
            ),
            Action::Layout
        );
        assert_eq!(app.popup, Some(Popup::Info));
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('i'),
                KeyModifiers::NONE
            ),
            Action::Layout
        );
        assert_eq!(app.popup, None);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('i'),
            KeyModifiers::SHIFT,
        );
        assert!(app.inspect_mode);
    }
    #[test]
    fn picker_can_filter_and_mark_multiple_images() {
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };
        let root = std::env::temp_dir().join(format!(
            "niv-picker-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("first_z.nii.gz"), b"").unwrap();
        fs::write(root.join("second_z.nii.gz"), b"").unwrap();
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "base".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('n'),
            KeyModifiers::NONE,
        );
        let filter = format!("{}/z.nii", root.display());
        for ch in filter.chars() {
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            );
        }
        assert_eq!(
            app.edit
                .as_ref()
                .unwrap()
                .completion
                .as_ref()
                .unwrap()
                .values
                .len(),
            2
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Down,
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        );
        let result = key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        match result {
            Action::LoadOverlays(paths) => assert_eq!(paths.len(), 2),
            other => panic!("{other:?}"),
        }
        // Keep the marks visible until the main loop finishes loading.
        assert_eq!(app.edit.as_ref().unwrap().checked.len(), 2);
        // With no explicit marks, Enter marks the highlighted file.
        app.edit.as_mut().unwrap().checked.clear();
        let result = key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        match result {
            Action::LoadOverlays(paths) => {
                assert_eq!(paths.len(), 1);
                assert_eq!(app.edit.as_ref().unwrap().checked, paths);
            }
            other => panic!("{other:?}"),
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn picker_zoxide_prompt_preserves_marks_and_cancel_restores_filter() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "base".into());
        let none = KeyModifiers::NONE;
        key(&mut app, &vol, &mut layers, KeyCode::Char('n'), none);
        key(&mut app, &vol, &mut layers, KeyCode::Char('f'), none);
        app.edit
            .as_mut()
            .unwrap()
            .checked
            .push(PathBuf::from("marked.nii.gz"));
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('z'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(app.picker_jump_from.as_deref(), Some("f"));
        for ch in "two words".chars() {
            key(&mut app, &vol, &mut layers, KeyCode::Char(ch), none);
        }
        assert_eq!(app.edit.as_ref().unwrap().text, "two words");
        key(&mut app, &vol, &mut layers, KeyCode::Esc, none);
        assert!(app.picker_jump_from.is_none());
        assert_eq!(app.edit.as_ref().unwrap().text, "f");
        assert_eq!(
            app.edit.as_ref().unwrap().checked,
            vec![PathBuf::from("marked.nii.gz")]
        );
        key(&mut app, &vol, &mut layers, KeyCode::Esc, none);
        assert!(app.edit.is_none());
    }

    #[test]
    fn picker_accepts_underscores_with_both_terminal_key_encodings() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "base".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('n'),
            KeyModifiers::NONE,
        );

        // Enhanced keyboard reporting can send the keycode plus Shift.
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('-'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.edit.as_ref().unwrap().text, "_");
        // Other terminals send the resulting character directly.
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('_'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.edit.as_ref().unwrap().text, "__");
        // A plain hyphen remains a hyphen.
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('-'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.edit.as_ref().unwrap().text, "__-");
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
    fn reordering_keeps_image_properties_and_selected_image() {
        use crate::layers::IntensityOverlay;
        let vol = example();
        let mk = |name: &str, window: (f32, f32), opacity: f32| IntensityOverlay {
            path: PathBuf::from(name),
            vol: example(),
            window,
            default_window: window,
            data_range: (0., 100.),
            palette: Palette::Solid([255, 50, 20]),
            visible: true,
            opacity,
            name: name.into(),
        };
        let mut layers = Layers {
            overlays: vec![mk("red", (2., 6.), 0.7), mk("yellow", (9., 15.), 0.3)],
            atlas: None,
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 99.), "base".into());
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('2'),
                KeyModifiers::NONE
            ),
            Action::Ui
        );
        assert_eq!(app.selected_layer_id(), 1);
        assert_eq!(app.selected_window(&layers), (2., 6.));

        // Raise the first overlay above the second using Shift+].
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char(']'),
                KeyModifiers::SHIFT
            ),
            Action::Redraw
        );
        assert_eq!(app.layer_order, vec![0, 2, 1]);
        assert_eq!(app.selected, 2);
        assert_eq!(app.selected_layer_id(), 1);
        assert_eq!(app.selected_window(&layers), (2., 6.));
        assert_eq!(app.selected_opacity(&layers), 0.7);
        assert_eq!(app.stride, 1); // shifted bracket did not change stride
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('}'),
                KeyModifiers::SHIFT
            ),
            Action::None
        );

        // Move above the base, then lower the base: both are reorderable.
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('3'),
                KeyModifiers::NONE
            ),
            Action::Ui
        );
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('}'),
                KeyModifiers::SHIFT
            ),
            Action::Redraw
        );
        assert_eq!(app.layer_order, vec![2, 0, 1]);
        assert_eq!(app.selected_layer_id(), 0);
        assert_eq!(app.selected_window(&layers), (0., 99.));

        // Selected image is followed when lowered via literal '{'.
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('{'),
                KeyModifiers::NONE
            ),
            Action::Redraw
        );
        assert_eq!(app.layer_order, vec![0, 2, 1]);
        assert_eq!(app.selected_layer_id(), 0);
        assert_eq!(app.selected, 0);
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('{'),
                KeyModifiers::SHIFT
            ),
            Action::None
        );

        // Editing position 2 edits image ID 2, not the previous position's ID.
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('2'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.selected_layer_id(), 2);
        assert_eq!(app.selected_window(&layers), (9., 15.));
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
            KeyCode::Char('1'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        assert_eq!(layers.overlays[1].window, (1., 15.));
        assert_eq!(layers.overlays[0].window, (2., 6.));

        // Loading another image preserves this order and appends it on top.
        layers.overlays.push(mk("new", (10., 20.), 1.));
        app.sync_layer_order(layers.overlays.len());
        assert_eq!(app.layer_order, vec![0, 2, 1, 3]);
    }

    #[test]
    fn layer_list_navigation_tracks_visual_order_and_enter_keeps_selection() {
        use crate::layers::IntensityOverlay;
        let vol = example();
        let mut layers = Layers {
            overlays: (1..=8)
                .map(|id| IntensityOverlay {
                    path: PathBuf::from(format!("image{id}.nii.gz")),
                    name: format!("image{id}"),
                    vol: example(),
                    window: (id as f32, 20.),
                    default_window: (id as f32, 20.),
                    data_range: (0., 20.),
                    palette: Palette::Gray,
                    visible: true,
                    opacity: 1.,
                })
                .collect(),
            atlas: None,
        };
        let mut app = App::new(Axis::Z, 30, 1, 0, (0., 20.), "base".into());
        app.sync_layer_order(layers.overlays.len());
        app.layer_order.swap(7, 8);
        let order = app.layer_order.clone();
        let none = KeyModifiers::NONE;
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('t'), none),
            Action::Layout
        );
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Down, none),
            Action::None
        );
        for _ in 0..8 {
            assert_eq!(
                key(&mut app, &vol, &mut layers, KeyCode::Up, none),
                Action::Ui
            );
        }
        assert_eq!(app.selected, 8);
        assert_eq!(app.selected_layer_id(), 7);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('k'), none),
            Action::None
        );
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Down, none),
            Action::Ui
        );
        assert_eq!(app.selected_layer_id(), 8);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('j'), none),
            Action::Ui
        );
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('k'), none),
            Action::Ui
        );
        assert_eq!(app.selected_layer_id(), 8);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Enter, none),
            Action::Layout
        );
        assert!(!app.show_layer_list);
        assert_eq!(app.selected_layer_id(), 8);
        assert_eq!(app.selected_window(&layers), (8., 20.));
        assert_eq!(app.layer_order, order);
        assert_eq!(app.index, 30);
        key(&mut app, &vol, &mut layers, KeyCode::Up, none);
        assert_eq!(app.index, 31);
        assert_eq!(app.selected_layer_id(), 8);
    }

    #[test]
    fn q_closes_menus_and_numeric_editors_before_quitting() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 30, 1, 0, (0., 20.), "base".into());
        let none = KeyModifiers::NONE;
        key(&mut app, &vol, &mut layers, KeyCode::Char('t'), none);
        key(&mut app, &vol, &mut layers, KeyCode::Char('?'), none);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('q'), none),
            Action::Layout
        );
        assert!(app.popup.is_none() && app.show_layer_list);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('q'), none),
            Action::Layout
        );
        assert!(!app.show_layer_list);
        let repeat = KeyEvent::new_with_kind(KeyCode::Char('q'), none, KeyEventKind::Repeat);
        assert_eq!(app.key(&vol, &mut layers, repeat), Action::None);
        key(&mut app, &vol, &mut layers, KeyCode::Char('m'), none);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('q'), none),
            Action::Layout
        );
        assert!(app.edit.is_none());
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('q'), none),
            Action::Quit
        );
    }

    #[test]
    fn ctrl_c_quits_in_every_input_context_and_q_remains_typable() {
        let vol = example();
        let mut layers = Layers::default();
        let none = KeyModifiers::NONE;
        for mode in [
            None,
            Some('t'),
            Some('?'),
            Some('I'),
            Some('m'),
            Some('n'),
            Some('s'),
        ] {
            let mut app = App::new(Axis::Z, 30, 1, 0, (0., 20.), "base".into());
            if let Some(c) = mode {
                key(&mut app, &vol, &mut layers, KeyCode::Char(c), none);
            }
            if matches!(mode, Some('n' | 's')) {
                let before = app.edit.as_ref().unwrap().text.clone();
                key(&mut app, &vol, &mut layers, KeyCode::Char('q'), none);
                assert_eq!(app.edit.as_ref().unwrap().text, format!("{before}q"));
            }
            assert_eq!(
                key(
                    &mut app,
                    &vol,
                    &mut layers,
                    KeyCode::Char('c'),
                    KeyModifiers::CONTROL
                ),
                Action::Quit
            );
        }
    }

    #[test]
    fn layer_list_does_not_capture_keys_when_hidden_by_fullscreen() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 30, 1, 0, (0., 20.), "base".into());
        let none = KeyModifiers::NONE;
        key(&mut app, &vol, &mut layers, KeyCode::Tab, none);
        assert!(app.fullscreen);
        key(&mut app, &vol, &mut layers, KeyCode::Char('t'), none);
        assert!(app.show_layer_list && !app.fullscreen);
        key(&mut app, &vol, &mut layers, KeyCode::Tab, none);
        assert!(app.fullscreen && !app.show_layer_list);
        key(&mut app, &vol, &mut layers, KeyCode::Up, none);
        assert_eq!(app.index, 31);
    }

    #[test]
    fn save_scene_opens_filename_editor_and_returns_path() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::X, 0, 1, 90, (0., 1.), "image_A".into());
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('s'),
                KeyModifiers::NONE
            ),
            Action::Ui
        );
        assert_eq!(app.edit.as_ref().unwrap().field, EditField::SaveScene);
        assert_eq!(app.edit.as_ref().unwrap().text, "image_A.niv.toml");
        for _ in 0..16 {
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Backspace,
                KeyModifiers::NONE,
            );
        }
        for c in "saved".chars() {
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
            KeyCode::Char('-'),
            KeyModifiers::SHIFT,
        );
        for c in "scene.niv.toml".chars() {
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char(c),
                KeyModifiers::NONE,
            );
        }
        let outcome = key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        assert_eq!(
            outcome,
            Action::SaveScene(PathBuf::from("saved_scene.niv.toml"))
        );
        assert!(app.edit.is_none());
    }

    #[test]
    fn base_visibility_can_toggle_without_changing_opacity() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 30, 1, 90, (0., 1.), "test".into());
        app.base_opacity = 0.4;
        assert!(app.layer_visible(&layers, 0));
        assert_eq!(
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char(' '),
                KeyModifiers::NONE
            ),
            Action::Redraw
        );
        assert!(!app.layer_visible(&layers, 0));
        assert_eq!(app.base_opacity, 0.4);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        );
        assert!(app.layer_visible(&layers, 0));
    }

    #[test]
    fn projection_toggles_do_not_change_default_slice_behavior() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 30, 1, 90, (0., 1.), "test".into());
        assert_eq!(app.projection, ProjectionMode::Slice);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('X'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.projection, ProjectionMode::Max);
        assert_eq!(app.projection_for(0), ProjectionMode::Max);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('a'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.projection_for(0), ProjectionMode::Mean);
        assert_eq!(app.projection_for(1), ProjectionMode::Max);
        // Select a second image by ID, then move it below the original base.
        layers.overlays.push(crate::layers::IntensityOverlay {
            path: "other.nii.gz".into(),
            name: "other".into(),
            vol: example(),
            window: (0., 1.),
            default_window: (0., 1.),
            data_range: (0., 0.),
            palette: Palette::Gray,
            visible: true,
            opacity: 1.,
        });
        app.sync_layer_order(1);
        app.selected = 1;
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('X'),
            KeyModifiers::NONE,
        );
        app.move_selected_layer(-1);
        assert_eq!(app.selected_layer_id(), 1);
        assert_eq!(app.projection_for(1), ProjectionMode::Max);
        assert_eq!(app.projection_for(0), ProjectionMode::Mean);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('v'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(app.projection, ProjectionMode::Slice);
        assert_eq!(app.projection_for(0), ProjectionMode::Slice);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('X'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.projection_for(0), ProjectionMode::Mean);
        app.projection = ProjectionMode::Slice;
        app.montage.enabled = true;
        let action = key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('X'),
            KeyModifiers::NONE,
        );
        assert_eq!(action, Action::Ui);
        assert_eq!(app.projection, ProjectionMode::Slice);
    }

    #[test]
    fn each_axis_retains_orientation_and_cut_paste_preserves_identity() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 30, 1, 90, (0., 1.), "test".into());
        let n = KeyModifiers::NONE;
        key(&mut app, &vol, &mut layers, KeyCode::Char('r'), n);
        key(&mut app, &vol, &mut layers, KeyCode::Char('f'), n);
        key(&mut app, &vol, &mut layers, KeyCode::Char('x'), n);
        assert_eq!((app.rotation, app.flip_h), (90, false));
        key(&mut app, &vol, &mut layers, KeyCode::Char('z'), n);
        assert_eq!((app.rotation, app.flip_h), (180, true));
        layers.overlays.push(crate::layers::IntensityOverlay {
            path: "other.nii.gz".into(),
            name: "other".into(),
            vol: example(),
            window: (0., 1.),
            default_window: (0., 1.),
            data_range: (0., 0.),
            palette: Palette::Gray,
            visible: true,
            opacity: 1.,
        });
        app.sync_layer_order(1);
        app.selected = 1;
        app.show_layer_list = true;
        key(&mut app, &vol, &mut layers, KeyCode::Char('X'), n);
        assert_eq!(app.cut_layer, Some(1));
        key(&mut app, &vol, &mut layers, KeyCode::Down, n);
        key(&mut app, &vol, &mut layers, KeyCode::Char('p'), n);
        assert_eq!(app.layer_order, vec![1, 0]);
        assert_eq!(app.selected_layer_id(), 1);
        assert_eq!(app.cut_layer, None);
    }

    #[test]
    fn top_numbering_and_new_shortcuts() {
        let vol = example();
        let mut layers = Layers::default();
        layers.overlays.push(crate::layers::IntensityOverlay {
            path: "template.nii.gz".into(),
            name: "template".into(),
            vol: example(),
            window: (0., 1.),
            default_window: (0., 1.),
            data_range: (0., 0.),
            palette: Palette::Gray,
            visible: true,
            opacity: 1.,
        });
        let mut app = App::new(Axis::X, 30, 1, 90, (0., 1.), "clusters".into());
        app.sync_layer_order(1);
        app.layer_order.reverse();
        app.selected = 1;
        let none = KeyModifiers::NONE;
        assert_eq!(app.layer_number(app.selected), 1);
        assert_eq!(app.selected_layer_id(), 0);
        key(&mut app, &vol, &mut layers, KeyCode::Char('2'), none);
        assert_eq!(app.selected_layer_id(), 1);
        key(&mut app, &vol, &mut layers, KeyCode::Char('w'), none);
        assert_eq!(app.layer_number(app.selected), 1);
        assert_eq!(app.selected_layer_id(), 1);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('d'), none),
            Action::UnloadLayer
        );
        key(&mut app, &vol, &mut layers, KeyCode::Char('{'), none);
        assert_eq!(app.layer_number(app.selected), 2);
        key(&mut app, &vol, &mut layers, KeyCode::Char('x'), none);
        assert!(app.flip_h);
        key(&mut app, &vol, &mut layers, KeyCode::Char('x'), none);
        assert!(!app.flip_h);
        key(&mut app, &vol, &mut layers, KeyCode::Char('c'), none);
        assert_eq!(app.axis, Axis::Y);
        key(&mut app, &vol, &mut layers, KeyCode::Char('c'), none);
        assert!(app.flip_h);
        key(&mut app, &vol, &mut layers, KeyCode::Char('y'), none);
        assert!(!app.flip_h);
        key(&mut app, &vol, &mut layers, KeyCode::Char('U'), none);
        assert_eq!(app.projection_for(1), ProjectionMode::MinPositive);
        assert!(!app.show_projected_atlas);
        for detail in [0, 1, 2, 0] {
            key(
                &mut app,
                &vol,
                &mut layers,
                KeyCode::Char('t'),
                KeyModifiers::SHIFT,
            );
            assert_eq!(app.projected_atlas_detail, detail);
            assert!(!app.show_layer_list);
            assert!(app.show_projected_atlas);
        }
        key(&mut app, &vol, &mut layers, KeyCode::Char('a'), none);
        assert!(!app.show_projected_atlas);
        assert!(app.show_atlas);
        key(&mut app, &vol, &mut layers, KeyCode::Char('a'), none);
        assert!(app.show_projected_atlas);
        key(&mut app, &vol, &mut layers, KeyCode::Char('V'), none);
        assert_eq!(
            key(&mut app, &vol, &mut layers, KeyCode::Char('u'), none),
            Action::CopyRegion
        );
    }

    #[test]
    fn range_editor_preserves_seven_decimal_input() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 30, 1, 0, (0., 1.), "test".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('m'),
            KeyModifiers::NONE,
        );
        for c in "0.9999001".chars() {
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
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('M'),
            KeyModifiers::SHIFT,
        );
        for c in "0.9999407".chars() {
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
        assert_eq!(app.window, (0.9999001_f32, 0.9999407_f32));
        assert_ne!(app.window.0, 0.99990_f32);
        assert_ne!(app.window.1, 0.99994_f32);
    }

    #[test]
    fn montage_exit_restores_slice_or_new_axis_midpoint() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 37, 1, 0, (0., 1.), "test".into());
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        );
        app.move_slices(&vol, 4);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.index, 37);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.index, vol.depth(Axis::X) / 2);
        // A montage restored from a scene has no session-only return point.
        app.montage.enabled = true;
        app.index = 0;
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.index, vol.depth(Axis::X) / 2);
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

    #[test]
    fn montage_controls_and_bounds() {
        let vol = example();
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 30, 1, 90, (0., 1.), "test".into());
        app.reset_montage(&vol);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        );
        assert!(app.montage.enabled);
        assert_eq!(app.montage.count(), 12);
        assert!(!app.montage.show_numbers);
        assert!(!app.montage.show_clusters);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('B'),
            KeyModifiers::SHIFT,
        );
        for c in ['2', '0'] {
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
        assert_eq!(app.montage.start, 20);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('e'),
            KeyModifiers::SHIFT,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Down,
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        assert_eq!(app.montage.stop, 89);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('N'),
            KeyModifiers::SHIFT,
        );
        assert!(app.montage.show_numbers);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('g'),
            KeyModifiers::SHIFT,
        );
        key(&mut app, &vol, &mut layers, KeyCode::Up, KeyModifiers::NONE);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Enter,
            KeyModifiers::NONE,
        );
        assert_eq!(app.atlas_blur, 0.25);
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        );
        assert_eq!(app.popup, Some(Popup::MontageHelp));
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Esc,
            KeyModifiers::NONE,
        );
        key(
            &mut app,
            &vol,
            &mut layers,
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        );
        assert!(!app.montage.enabled);
    }
}

#[cfg(test)]
mod layer_list_escape_tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ndarray::Array3;

    #[test]
    fn escape_closes_list_then_exits() {
        let volume = Volume {
            data: Array3::zeros((4, 4, 4)),
            spacing: [1.0; 3],
            sform: None,
            datatype: 16,
        };
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0.0, 1.0), "image".into());
        assert_eq!(
            app.key(
                &volume,
                &mut layers,
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)
            ),
            Action::Layout
        );
        assert!(app.show_layer_list);
        assert_eq!(
            app.key(
                &volume,
                &mut layers,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Layout
        );
        assert!(!app.show_layer_list);
        assert_eq!(
            app.key(
                &volume,
                &mut layers,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Quit
        );
    }

    #[test]
    fn escape_closes_popup_before_layer_list() {
        let volume = Volume {
            data: Array3::zeros((4, 4, 4)),
            spacing: [1.0; 3],
            sform: None,
            datatype: 16,
        };
        let mut layers = Layers::default();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0.0, 1.0), "image".into());
        app.key(
            &volume,
            &mut layers,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        );
        app.key(
            &volume,
            &mut layers,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert!(app.popup.is_some());
        assert_eq!(
            app.key(
                &volume,
                &mut layers,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Layout
        );
        assert!(app.popup.is_none());
        assert!(app.show_layer_list);
        assert_eq!(
            app.key(
                &volume,
                &mut layers,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Layout
        );
        assert!(!app.show_layer_list);
        assert_eq!(
            app.key(
                &volume,
                &mut layers,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Quit
        );
    }
}
