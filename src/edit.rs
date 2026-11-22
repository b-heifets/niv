//! Experimental, non-destructive three-plane NIfTI segmentation editor.
//! `niv edit --masked IMAGE --mask MASK [--raw IMAGE] [--out DIR]`.
//! All coordinates are original voxel indices; this mode never reorients data.
use crate::{kitty, render::Geometry, volume::Volume};
use anyhow::{bail, Context, Result};
use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use ndarray::Array3;
use nifti::writer::WriterOptions;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const HEADER_ROWS: u16 = 2;
const FOOTER_ROWS: u16 = 3;
const MAX_HISTORY: usize = 64;
const MAX_RADIUS_UM: f32 = 10_000.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Trim,
    Fill,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Trim => "Trim",
            Self::Fill => "Fill",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Paint,
    Erase,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Fill,
    Window,
}
#[derive(Clone)]
struct Segment {
    role: Role,
    name: String,
    color: [u8; 3],
    voxels: Array3<u8>,
    fill_value: f32,
}
#[derive(Serialize, Deserialize)]
struct SegmentMetadata {
    version: u32,
    role: String,
    fill_value: f32,
    color: [u8; 3],
}
fn metadata_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.toml", path.display()))
}
struct Delta {
    seg: usize,
    changes: Vec<(usize, u8, u8)>,
}
struct Editor {
    source: Volume,
    source_path: PathBuf,
    mask_source: Volume,
    mask_path: PathBuf,
    raw: Option<Volume>,
    work: Array3<f32>,
    binary: Array3<u8>,
    segments: Vec<Segment>,
    active_seg: usize,
    coordinates: [usize; 3],
    active_view: usize,
    solo: bool,
    zoom: f32,
    brush_um: f32,
    brush_offsets: Vec<[isize; 3]>,
    tool: Tool,
    show_raw: bool,
    show_mask: bool,
    overlay_opacity: f32,
    windows: (f32, f32),
    out_dir: PathBuf,
    overwrite: bool,
    saved_once: bool,
    undo: Vec<Delta>,
    redo: Vec<Delta>,
    pending: HashMap<usize, u8>,
    hover: Option<[usize; 3]>,
    dirty: bool,
    notice: String,
    quit_armed: bool,
    edit_value: Option<(EntryKind, String)>,
}

#[derive(Debug)]
struct Args {
    masked: PathBuf,
    mask: PathBuf,
    raw: Option<PathBuf>,
    out: Option<PathBuf>,
    trims: Vec<PathBuf>,
    fills: Vec<PathBuf>,
    radius_um: f32,
    overwrite: bool,
}

fn edit_usage() -> &'static str {
    r#"niv edit — experimental NIfTI mask/segmentation editor

Usage: niv edit --masked autofluo_50um_masked.nii.gz \
              --mask autofluo_50um_brain_mask.nii.gz \
              [--raw autofluo_50um.nii.gz] [--out DIRECTORY] \
              [--trim previous_trim.nii.gz]... [--fill previous_fill.nii.gz]...
              [--radius-um 150] [--overwrite]

All files must share the same voxel grid. Original inputs are never modified.
Output contains a derived masked image, a binary brain mask and individual
trim/fill segmentations with original spatial metadata. Press ? in the editor
for keyboard and mouse controls. --overwrite permits replacing outputs.
"#
}
fn parse_args(args: &[String]) -> Result<Option<Args>> {
    let mut masked = None;
    let mut mask = None;
    let mut raw = None;
    let mut out = None;
    let mut trims = Vec::new();
    let mut fills = Vec::new();
    let mut radius_um = 150f32;
    let mut overwrite = false;
    let mut i = 0;
    while i < args.len() {
        let opt = &args[i];
        if opt == "-h" || opt == "--help" {
            print!("{}", edit_usage());
            return Ok(None);
        }
        if opt == "--overwrite" {
            overwrite = true;
            i += 1;
            continue;
        }
        let next = args
            .get(i + 1)
            .with_context(|| format!("{opt} requires an argument"))?;
        match opt.as_str() {
            "--masked" => masked = Some(PathBuf::from(next)),
            "--mask" => mask = Some(PathBuf::from(next)),
            "--raw" => raw = Some(PathBuf::from(next)),
            "--out" => out = Some(PathBuf::from(next)),
            "--trim" => trims.push(PathBuf::from(next)),
            "--fill" => fills.push(PathBuf::from(next)),
            "--radius-um" => {
                radius_um = next
                    .parse::<f32>()
                    .with_context(|| format!("Invalid radius {next}"))?
            }
            _ => bail!("Unknown edit option: {opt}\n{}", edit_usage()),
        }
        i += 2;
    }
    if !radius_um.is_finite() || !(1.0..=MAX_RADIUS_UM).contains(&radius_um) {
        bail!("Brush radius must be between 1 and {MAX_RADIUS_UM} µm");
    }
    Ok(Some(Args {
        masked: masked.context("Missing --masked")?,
        mask: mask.context("Missing --mask")?,
        raw,
        out,
        trims,
        fills,
        radius_um,
        overwrite,
    }))
}

pub fn maybe_run() -> Result<bool> {
    let args: Vec<String> = env::args().collect();
    if !args.get(1).is_some_and(|a| a == "edit" || a == "--edit") {
        return Ok(false);
    }
    if let Some(options) = parse_args(&args[2..])? {
        run(options)?;
    }
    Ok(true)
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
        .trim_end_matches(".gz")
        .trim_end_matches(".nii")
        .to_string()
}
fn palette(role: Role, i: usize) -> [u8; 3] {
    let trim = [[255, 104, 108], [255, 175, 70], [255, 92, 167]];
    let fill = [
        [53, 221, 196],
        [111, 189, 255],
        [199, 143, 255],
        [181, 227, 76],
    ];
    match role {
        Role::Trim => trim[i % trim.len()],
        Role::Fill => fill[i % fill.len()],
    }
}
fn default_output_path(masked: &Path) -> PathBuf {
    let parent = masked.parent().unwrap_or(Path::new("."));
    let base = name(masked);
    for n in 1..=999 {
        let path = parent.join(if n == 1 {
            format!("{base}_niv_edits")
        } else {
            format!("{base}_niv_edits_{n}")
        });
        if !path.exists() {
            return path;
        }
    }
    parent.join(format!("{base}_niv_edits_new"))
}
fn flat(dims: [usize; 3], pos: [usize; 3]) -> usize {
    (pos[0] * dims[1] + pos[1]) * dims[2] + pos[2]
}
fn from_flat(dims: [usize; 3], index: usize) -> [usize; 3] {
    [
        index / (dims[1] * dims[2]),
        (index / dims[2]) % dims[1],
        index % dims[2],
    ]
}
fn physical_scale(source: &Volume) -> f32 {
    source.micrometre_scale().unwrap_or(1000.) as f32
}
fn sphere_offsets(spacing: [f32; 3], um_scale: f32, radius: f32) -> Vec<[isize; 3]> {
    let xyz = spacing.map(|s| (s * um_scale).max(0.001));
    let range = xyz.map(|s| (radius / s).ceil().max(0.) as isize);
    let mut offsets = Vec::new();
    for x in -range[0]..=range[0] {
        for y in -range[1]..=range[1] {
            for z in -range[2]..=range[2] {
                let d2 = (x as f32 * xyz[0]).powi(2)
                    + (y as f32 * xyz[1]).powi(2)
                    + (z as f32 * xyz[2]).powi(2);
                if d2 <= radius * radius + 1e-3 {
                    offsets.push([x, y, z]);
                }
            }
        }
    }
    offsets
}

impl Editor {
    fn load(args: Args) -> Result<Self> {
        let source = Volume::load(&args.masked)?;
        let mask_source = Volume::load(&args.mask)?;
        source
            .validate_grid(&mask_source)
            .context("Brain mask grid does not match the masked autofluorescence image")?;
        let raw = if let Some(path) = args.raw.as_ref() {
            let vol = Volume::load(path)?;
            source
                .validate_grid(&vol)
                .context("Raw autofluorescence grid does not match")?;
            Some(vol)
        } else {
            None
        };
        let dims = source.dims();
        let binary = mask_source
            .data
            .mapv(|x| u8::from(x.is_finite() && x > 0.5));
        let work = source.data.clone();
        let out_dir = args
            .out
            .unwrap_or_else(|| default_output_path(&args.masked));
        let mut editor = Self {
            windows: source.auto_window(),
            coordinates: dims.map(|n| n / 2),
            brush_offsets: sphere_offsets(source.spacing, physical_scale(&source), args.radius_um),
            brush_um: args.radius_um,
            source,
            source_path: args.masked,
            mask_source,
            mask_path: args.mask,
            raw,
            work,
            binary,
            segments: Vec::new(),
            active_seg: 0,
            active_view: 0,
            solo: false,
            zoom: 1.,
            tool: Tool::Paint,
            show_raw: false,
            show_mask: true,
            overlay_opacity: 0.45,
            out_dir,
            overwrite: args.overwrite,
            saved_once: false,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: HashMap::new(),
            hover: None,
            dirty: false,
            quit_armed: false,
            notice: String::new(),
            edit_value: None,
        };
        for path in args.trims {
            editor.load_segment(Role::Trim, &path)?;
        }
        for path in args.fills {
            editor.load_segment(Role::Fill, &path)?;
        }
        if editor.segments.is_empty() {
            editor.new_segment(Role::Trim);
        }
        editor.active_seg = editor.segments.len() - 1;
        editor.rebuild_all();
        if editor.source.micrometre_scale().is_none() {
            editor.notice =
                "NIfTI spatial units unspecified; assuming voxel spacing is in mm".into();
        }
        Ok(editor)
    }
    fn load_segment(&mut self, role: Role, path: &Path) -> Result<()> {
        let vol = Volume::load(path)?;
        self.source.validate_grid(&vol)?;
        let count = self.segments.iter().filter(|s| s.role == role).count();
        let metadata = if metadata_path(path).is_file() {
            let metadata: SegmentMetadata =
                toml::from_str(&fs::read_to_string(metadata_path(path))?)?;
            if metadata.version != 1 || metadata.role.to_lowercase() != role.name().to_lowercase() {
                bail!(
                    "Segmentation metadata at {} does not match the selected --{} role",
                    metadata_path(path).display(),
                    role.name().to_lowercase()
                );
            }
            if !metadata.fill_value.is_finite() || metadata.fill_value < 0. {
                bail!("Invalid saved fill intensity");
            }
            Some(metadata)
        } else {
            None
        };
        let fill_value = metadata
            .as_ref()
            .map(|m| m.fill_value)
            .unwrap_or_else(|| self.default_fill_value());
        let color = metadata
            .as_ref()
            .map(|m| m.color)
            .unwrap_or_else(|| palette(role, count));
        let stem = name(path);
        let stem = if stem
            .as_bytes()
            .get(0..3)
            .is_some_and(|p| p[0].is_ascii_digit() && p[1].is_ascii_digit() && p[2] == b'_')
        {
            stem[3..].to_string()
        } else {
            stem
        };
        self.segments.push(Segment {
            role,
            name: stem,
            color,
            voxels: vol.data.mapv(|v| u8::from(v.is_finite() && v > 0.5)),
            fill_value,
        });
        Ok(())
    }
    fn default_fill_value(&self) -> f32 {
        let center = self.coordinates;
        let v = self.source.data[center];
        if v.is_finite() && v > 0. {
            return v;
        }
        self.source
            .data
            .iter()
            .copied()
            .find(|v| v.is_finite() && *v > 0.)
            .unwrap_or(1.)
    }
    fn new_segment(&mut self, role: Role) {
        self.finish_stroke();
        let count = self.segments.iter().filter(|s| s.role == role).count() + 1;
        let shape = self.source.data.raw_dim();
        let fill_value = self.default_fill_value();
        self.segments.push(Segment {
            role,
            name: format!("{}_{count:02}", role.name().to_lowercase()),
            color: palette(role, count - 1),
            voxels: Array3::zeros(shape),
            fill_value,
        });
        self.active_seg = self.segments.len() - 1;
        self.notice = format!(
            "New {} segment · {}",
            role.name(),
            self.segments[self.active_seg].name
        );
    }
    fn dims(&self) -> [usize; 3] {
        self.source.dims()
    }
    fn recompute(&mut self, pos: [usize; 3]) {
        let mut value = self.source.data[pos];
        let mut inside = u8::from(self.mask_source.data[pos] > 0.5);
        for seg in &self.segments {
            if seg.voxels[pos] == 0 {
                continue;
            }
            match seg.role {
                Role::Trim => {
                    value = 0.;
                    inside = 0;
                }
                Role::Fill => {
                    value = seg.fill_value;
                    inside = 1;
                }
            }
        }
        self.work[pos] = value;
        self.binary[pos] = inside;
    }
    fn rebuild_all(&mut self) {
        let dims = self.dims();
        for x in 0..dims[0] {
            for y in 0..dims[1] {
                for z in 0..dims[2] {
                    self.recompute([x, y, z]);
                }
            }
        }
    }
    fn begin_stroke(&mut self) {
        self.finish_stroke();
    }
    fn paint_sphere(&mut self, center: [usize; 3], erase: bool) -> usize {
        let dims = self.dims();
        let mut edited = 0;
        let value = u8::from(!erase);
        let seg = self.active_seg;
        for i in 0..self.brush_offsets.len() {
            let offset = self.brush_offsets[i];
            let p = [
                center[0] as isize + offset[0],
                center[1] as isize + offset[1],
                center[2] as isize + offset[2],
            ];
            if (0..3).any(|a| p[a] < 0 || p[a] >= dims[a] as isize) {
                continue;
            }
            let pos = [p[0] as usize, p[1] as usize, p[2] as usize];
            let before = self.segments[seg].voxels[pos];
            if before == value {
                continue;
            }
            let id = flat(dims, pos);
            self.pending.entry(id).or_insert(before);
            self.segments[seg].voxels[pos] = value;
            self.recompute(pos);
            edited += 1;
        }
        if edited > 0 {
            self.dirty = true;
            self.quit_armed = false;
        }
        edited
    }
    fn finish_stroke(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let seg = self.active_seg;
        let dims = self.dims();
        let changes = std::mem::take(&mut self.pending)
            .into_iter()
            .filter_map(|(i, before)| {
                let after = self.segments[seg].voxels[from_flat(dims, i)];
                (before != after).then_some((i, before, after))
            })
            .collect::<Vec<_>>();
        if !changes.is_empty() {
            self.undo.push(Delta { seg, changes });
            if self.undo.len() > MAX_HISTORY {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }
    fn apply_delta(&mut self, d: &Delta, redo: bool) {
        let dims = self.dims();
        for &(i, before, after) in &d.changes {
            let p = from_flat(dims, i);
            self.segments[d.seg].voxels[p] = if redo { after } else { before };
            self.recompute(p);
        }
        self.dirty = true;
    }
    fn undo(&mut self) {
        self.finish_stroke();
        if let Some(delta) = self.undo.pop() {
            self.apply_delta(&delta, false);
            self.redo.push(delta);
            self.notice = "Undo stroke".into();
        }
    }
    fn redo(&mut self) {
        self.finish_stroke();
        if let Some(delta) = self.redo.pop() {
            self.apply_delta(&delta, true);
            self.undo.push(delta);
            self.notice = "Redo stroke".into();
        }
    }
    fn radius(&mut self, delta: f32) {
        self.brush_um = (self.brush_um + delta).clamp(1., MAX_RADIUS_UM);
        self.brush_offsets = sphere_offsets(
            self.source.spacing,
            physical_scale(&self.source),
            self.brush_um,
        );
    }
    fn change_zoom(&mut self, delta: f32) {
        self.zoom = (self.zoom + delta).clamp(1., 8.);
    }
    fn move_slice(&mut self, axis: usize, delta: isize) -> bool {
        let previous = self.coordinates[axis];
        self.coordinates[axis] = (previous as isize + delta)
            .clamp(0, self.dims()[axis].saturating_sub(1) as isize)
            as usize;
        previous != self.coordinates[axis]
    }
    fn sample_fill(&mut self) {
        let p = self.hover.unwrap_or(self.coordinates);
        let source = self.work[p];
        let value = if source > 0. && source.is_finite() {
            source
        } else {
            let dims = self.dims();
            let mut nearest = None;
            'search: for rad in 1isize..=5 {
                for dx in -rad..=rad {
                    for dy in -rad..=rad {
                        for dz in -rad..=rad {
                            let a = [p[0] as isize + dx, p[1] as isize + dy, p[2] as isize + dz];
                            if (0..3).any(|k| a[k] < 0 || a[k] >= dims[k] as isize) {
                                continue;
                            }
                            let v = self.work[[a[0] as usize, a[1] as usize, a[2] as usize]];
                            if v.is_finite() && v > 0. {
                                nearest = Some(v);
                                break 'search;
                            }
                        }
                    }
                }
            }
            nearest.unwrap_or(self.default_fill_value())
        };
        if self.segments[self.active_seg].role == Role::Fill {
            self.segments[self.active_seg].fill_value = value;
            self.rebuild_all();
            self.dirty = true;
            self.notice = format!("Fill intensity = {value:.5} (sampled near cursor)");
        } else {
            self.notice = "Select a Fill segment first (N)".into();
        }
    }
    fn output_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            self.out_dir
                .join(format!("{}_niv_edited.nii.gz", name(&self.source_path))),
            self.out_dir
                .join(format!("{}_niv_edited.nii.gz", name(&self.mask_path))),
        ];
        for (n, seg) in self.segments.iter().enumerate() {
            paths.push(
                self.out_dir
                    .join(format!("{:02}_{}.nii.gz", n + 1, seg.name)),
            );
        }
        paths
    }
    fn save(&mut self) -> Result<()> {
        self.finish_stroke();
        let paths = self.output_paths();
        if !self.overwrite && !self.saved_once && paths.iter().any(|p| p.exists()) {
            bail!(
                "Output files already exist in {}. Use --out NEW_DIRECTORY or --overwrite",
                self.out_dir.display()
            );
        }
        fs::create_dir_all(&self.out_dir)?;
        // Write compressed NIfTI images using the ORIGINAL file headers so voxel
        // spacing, orientation metadata and alignment remain unchanged.
        for (i, path) in paths.iter().enumerate() {
            let tmp = self.out_dir.join(format!(".niv_{}.part.nii.gz", i));
            let result = (|| -> Result<()> {
                match i {
                    0 => WriterOptions::new(&tmp)
                        .reference_file(&self.source_path)
                        .write_nifti(&self.work)?,
                    1 => WriterOptions::new(&tmp)
                        .reference_file(&self.mask_path)
                        .write_nifti(&self.binary)?,
                    _ => WriterOptions::new(&tmp)
                        .reference_file(&self.mask_path)
                        .write_nifti(&self.segments[i - 2].voxels)?,
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            result.with_context(|| format!("Writing {}", path.display()))?;
            fs::rename(&tmp, path).with_context(|| format!("Saving {}", path.display()))?;
            if i >= 2 {
                let seg = &self.segments[i - 2];
                let info = SegmentMetadata {
                    version: 1,
                    role: seg.role.name().to_lowercase(),
                    fill_value: seg.fill_value,
                    color: seg.color,
                };
                let content = toml::to_string_pretty(&info)?;
                let info_path = metadata_path(path);
                let tmp_info = metadata_path(&tmp);
                fs::write(&tmp_info, content)?;
                fs::rename(&tmp_info, &info_path)?;
            }
        }
        self.saved_once = true;
        self.dirty = false;
        self.quit_armed = false;
        self.notice = format!(
            "Saved {} NIfTI files in {}",
            paths.len(),
            self.out_dir.display()
        );
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Pane {
    axis: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}
#[derive(Clone)]
struct Canvas {
    width: usize,
    height: usize,
    geom: Geometry,
    panes: Vec<Pane>,
    cols: u16,
    rows: u16,
}
fn axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}
fn source_aspect(vol: &Volume, axis: usize) -> f64 {
    let (h, v) = axes(axis);
    vol.dims()[h] as f64 * vol.spacing[h] as f64 / (vol.dims()[v] as f64 * vol.spacing[v] as f64)
}
fn canvas(vol: &Volume, editor: &Editor) -> Result<Canvas> {
    let (cols, rows) = terminal::size()?;
    if cols < 28 || rows < 12 {
        bail!("Terminal too small for segmentation editing (minimum 28 columns, 12 rows)");
    }
    let (cw, ch) = crate::render::cell_pixels(cols, rows);
    let cell_rows = rows.saturating_sub(HEADER_ROWS + FOOTER_ROWS);
    let full_width = cols as usize * cw;
    let full_height = cell_rows as usize * ch;
    let budget = env::var("NIV_EDIT_PIXEL_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1_000_000);
    let scale = if budget > 0 && full_width.saturating_mul(full_height) > budget {
        (budget as f64 / (full_width * full_height) as f64).sqrt()
    } else {
        1.
    };
    let width = ((full_width as f64 * scale).round() as usize).max(1);
    let height = ((full_height as f64 * scale).round() as usize).max(1);
    let geom = Geometry {
        cols,
        rows: cell_rows,
        width,
        height,
    };
    let mut panes = Vec::new();
    let tiles = if editor.solo {
        vec![editor.active_view]
    } else {
        vec![0, 1, 2]
    };
    let margin = 6usize.min(width / 20).min(height / 20);
    for (n, &axis) in tiles.iter().enumerate() {
        let col_start = n * width / tiles.len();
        let col_end = (n + 1) * width / tiles.len();
        let available_w = col_end.saturating_sub(col_start + 2 * margin).max(1);
        let available_h = height.saturating_sub(2 * margin).max(1);
        let ratio = source_aspect(vol, axis).clamp(0.01, 100.);
        let (w, h) = if available_w as f64 / available_h as f64 > ratio {
            (
                ((available_h as f64 * ratio).round() as usize).max(1),
                available_h,
            )
        } else {
            (
                available_w,
                ((available_w as f64 / ratio).round() as usize).max(1),
            )
        };
        panes.push(Pane {
            axis,
            x: col_start + (col_end - col_start).saturating_sub(w) / 2,
            y: (height - h) / 2,
            w,
            h,
        });
    }
    Ok(Canvas {
        width,
        height,
        geom,
        panes,
        cols,
        rows,
    })
}
fn plane_center(editor: &Editor, axis: usize) -> (f64, f64) {
    let (h, v) = axes(axis);
    let dims = editor.dims();
    if editor.zoom <= 1.001 {
        ((dims[h] as f64 - 1.) / 2., (dims[v] as f64 - 1.) / 2.)
    } else {
        (editor.coordinates[h] as f64, editor.coordinates[v] as f64)
    }
}
fn voxel_at(editor: &Editor, pane: &Pane, x: usize, y: usize) -> Option<[usize; 3]> {
    if x < pane.x || x >= pane.x + pane.w || y < pane.y || y >= pane.y + pane.h {
        return None;
    }
    let (ha, va) = axes(pane.axis);
    let dims = editor.dims();
    let (center_h, center_v) = plane_center(editor, pane.axis);
    let span_h = dims[ha] as f64 / editor.zoom as f64;
    let span_v = dims[va] as f64 / editor.zoom as f64;
    let h = (center_h + ((x - pane.x) as f64 + 0.5) / pane.w as f64 * span_h - span_h / 2.).round()
        as isize;
    let v = (center_v + span_v / 2. - ((y - pane.y) as f64 + 0.5) / pane.h as f64 * span_v).round()
        as isize;
    if h < 0 || h >= dims[ha] as isize || v < 0 || v >= dims[va] as isize {
        return None;
    }
    let mut xyz = editor.coordinates;
    xyz[ha] = h as usize;
    xyz[va] = v as usize;
    Some(xyz)
}
fn pixel_for(editor: &Editor, pane: &Pane, point: [usize; 3]) -> (isize, isize) {
    let (ha, va) = axes(pane.axis);
    let dims = editor.dims();
    let (ch, cv) = plane_center(editor, pane.axis);
    let sx = dims[ha] as f64 / editor.zoom as f64;
    let sy = dims[va] as f64 / editor.zoom as f64;
    let px = pane.x as f64 + pane.w as f64 * ((point[ha] as f64 - ch) / sx + 0.5);
    let py = pane.y as f64 + pane.h as f64 * (0.5 - (point[va] as f64 - cv) / sy);
    (px.round() as isize, py.round() as isize)
}
fn blend(dst: &mut [u8], fg: [u8; 3], alpha: f32) {
    let a = alpha.clamp(0., 1.);
    for (d, f) in dst.iter_mut().zip(fg) {
        *d = (*d as f32 * (1. - a) + f as f32 * a).round() as u8;
    }
}
fn put(rgb: &mut [u8], width: usize, height: usize, x: isize, y: isize, color: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
        let offset = (y as usize * width + x as usize) * 3;
        rgb[offset..offset + 3].copy_from_slice(&color);
    }
}
fn colorized(editor: &Editor, pos: [usize; 3]) -> [u8; 3] {
    let value = if editor.show_raw {
        editor
            .raw
            .as_ref()
            .map(|r| r.data[pos])
            .unwrap_or(editor.work[pos])
    } else {
        editor.work[pos]
    };
    let (lo, hi) = editor.windows;
    let grey = if value.is_finite() {
        ((value - lo) / (hi - lo).max(f32::EPSILON) * 255.).clamp(0., 255.) as u8
    } else {
        0
    };
    let mut rgb = [grey; 3];
    if editor.show_mask && editor.binary[pos] > 0 {
        blend(&mut rgb, [59, 160, 115], 0.10);
    }
    for (i, seg) in editor.segments.iter().enumerate() {
        if seg.voxels[pos] > 0 {
            blend(
                &mut rgb,
                seg.color,
                editor.overlay_opacity * if i == editor.active_seg { 1. } else { 0.70 },
            );
        }
    }
    rgb
}
fn render_canvas(editor: &Editor, canvas: &Canvas) -> Vec<u8> {
    let mut rgb = vec![0u8; canvas.width * canvas.height * 3];
    for pane in &canvas.panes {
        for y in pane.y..pane.y + pane.h {
            for x in pane.x..pane.x + pane.w {
                if let Some(voxel) = voxel_at(editor, pane, x, y) {
                    let idx = (y * canvas.width + x) * 3;
                    rgb[idx..idx + 3].copy_from_slice(&colorized(editor, voxel));
                }
            }
        }
        let border = if pane.axis == editor.active_view {
            [104, 222, 231]
        } else {
            [78, 82, 95]
        };
        for x in pane.x..pane.x + pane.w {
            put(
                &mut rgb,
                canvas.width,
                canvas.height,
                x as isize,
                pane.y as isize,
                border,
            );
            put(
                &mut rgb,
                canvas.width,
                canvas.height,
                x as isize,
                (pane.y + pane.h - 1) as isize,
                border,
            );
        }
        for y in pane.y..pane.y + pane.h {
            put(
                &mut rgb,
                canvas.width,
                canvas.height,
                pane.x as isize,
                y as isize,
                border,
            );
            put(
                &mut rgb,
                canvas.width,
                canvas.height,
                (pane.x + pane.w - 1) as isize,
                y as isize,
                border,
            );
        }
        // Display both orthogonal intersection lines on every pane. Their
        // voxel locations remain synchronized as the active slice scrolls.
        let (cross_x, cross_y) = pixel_for(editor, pane, editor.coordinates);
        if cross_x >= pane.x as isize && cross_x < (pane.x + pane.w) as isize {
            for y in pane.y + 1..pane.y + pane.h.saturating_sub(1) {
                let k = (y * canvas.width + cross_x as usize) * 3;
                blend(&mut rgb[k..k + 3], [168, 208, 238], 0.32);
            }
        }
        if cross_y >= pane.y as isize && cross_y < (pane.y + pane.h) as isize {
            for x in pane.x + 1..pane.x + pane.w.saturating_sub(1) {
                let k = (cross_y as usize * canvas.width + x) * 3;
                blend(&mut rgb[k..k + 3], [168, 208, 238], 0.32);
            }
        }
        // Show the 3D brush's planar cross section in the currently hovered
        // pane; this preview does not modify voxel data.
        if pane.axis == editor.active_view {
            let center = editor.hover.unwrap_or(editor.coordinates);
            let (px, py) = pixel_for(editor, pane, center);
            let (ha, va) = axes(pane.axis);
            let scale = physical_scale(&editor.source);
            let rx =
                editor.brush_um / (editor.source.spacing[ha] * scale) * pane.w as f32 * editor.zoom
                    / editor.dims()[ha] as f32;
            let ry =
                editor.brush_um / (editor.source.spacing[va] * scale) * pane.h as f32 * editor.zoom
                    / editor.dims()[va] as f32;
            let (rx, ry) = (rx.clamp(1., 1000.), ry.clamp(1., 1000.));
            let n = (((rx + ry) * 2.) as usize).clamp(24, 360);
            for k in 0..n {
                let t = k as f32 * std::f32::consts::TAU / n as f32;
                let x = px + (t.cos() * rx).round() as isize;
                let y = py + (t.sin() * ry).round() as isize;
                if x > pane.x as isize
                    && y > pane.y as isize
                    && x < (pane.x + pane.w - 1) as isize
                    && y < (pane.y + pane.h - 1) as isize
                {
                    put(
                        &mut rgb,
                        canvas.width,
                        canvas.height,
                        x,
                        y,
                        editor.segments[editor.active_seg].color,
                    );
                }
            }
        }
    }
    rgb
}
fn cut(s: &str, width: usize) -> String {
    s.chars().take(width).collect()
}
fn draw(
    editor: &Editor,
    out: &mut impl Write,
    active: &mut Option<u32>,
    next: &mut u32,
) -> Result<Canvas> {
    let canvas = canvas(&editor.source, editor)?;
    let frame = kitty::encode_pixels(&render_canvas(editor, &canvas), false)?;
    queue!(
        out,
        MoveTo(0, 0),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Cyan),
        Print(cut(
            &format!(
                "niv EDIT | X Y Z : [{},{},{}] | active {} | {} | {} | {}",
                editor.coordinates[0],
                editor.coordinates[1],
                editor.coordinates[2],
                ["X", "Y", "Z"][editor.active_view],
                editor.segments[editor.active_seg].name,
                if editor.tool == Tool::Paint {
                    "paint"
                } else {
                    "erase"
                },
                if editor.solo {
                    "single view"
                } else {
                    "3 views"
                }
            ),
            canvas.cols as usize
        )),
        ResetColor
    )?;
    let hover = editor.hover.unwrap_or(editor.coordinates);
    let v = editor.work[hover];
    let mask = editor.binary[hover];
    let seg = &editor.segments[editor.active_seg];
    queue!(out,MoveTo(0,1),Clear(ClearType::CurrentLine),SetForegroundColor(Color::Grey),
        Print(cut(&format!("Hover [{},{},{}] : {:.5}  mask {} | {} {} | radius {:.0} µm | zoom {:.2}× | ? help",
            hover[0],hover[1],hover[2],v,mask,seg.role.name(),
            if seg.role==Role::Fill{format!("intensity {:.5}",seg.fill_value)}else{"zero outside".into()},
            editor.brush_um,editor.zoom),canvas.cols as usize)),ResetColor)?;
    queue!(out, MoveTo(0, HEADER_ROWS))?;
    let id = *next;
    *next = next.wrapping_add(1).max(1);
    kitty::replace(out, id, &frame, canvas.geom, 0, *active)?;
    *active = Some(id);
    let footer = HEADER_ROWS + canvas.geom.rows;
    let current = editor
        .segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}{}:{}",
                if i == editor.active_seg { ">" } else { "" },
                i + 1,
                s.name
            )
        })
        .collect::<Vec<_>>()
        .join("  ");
    queue!(
        out,
        MoveTo(0, footer),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Yellow),
        Print(cut(&format!("Segments: {current}"), canvas.cols as usize)),
        ResetColor
    )?;
    queue!(out,MoveTo(0,footer+1),Clear(ClearType::CurrentLine),SetForegroundColor(Color::Grey),
        Print(cut("1/2/3 choose view  Tab cycle  ↑↓ scroll  ←→ crosshair  f fill view  [ ] brush  +/- zoom  mouse paint/erase  s save",canvas.cols as usize)),ResetColor)?;
    let msg = if let Some((kind, txt)) = editor.edit_value.as_ref() {
        format!(
            "{} (Enter apply, Esc cancel): {txt}",
            if *kind == EntryKind::Fill {
                "Fill intensity"
            } else {
                "Window min,max"
            }
        )
    } else if editor.quit_armed {
        "Unsaved edits: press q again to quit without saving, or s to save".to_string()
    } else if editor.notice.is_empty() {
        format!(
            "{} | {} | {} | out: {}",
            if editor.dirty {
                "UNSAVED"
            } else {
                "saved / unchanged"
            },
            if editor.show_raw { "raw" } else { "masked" },
            if editor.show_mask {
                "mask visible"
            } else {
                "mask hidden"
            },
            editor.out_dir.display()
        )
    } else {
        editor.notice.clone()
    };
    queue!(
        out,
        MoveTo(0, footer + 2),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(cut(&msg, canvas.cols as usize)),
        ResetColor
    )?;
    out.flush()?;
    Ok(canvas)
}
fn mouse_voxel(editor: &Editor, canvas: &Canvas, mouse: MouseEvent) -> Option<(usize, [usize; 3])> {
    let (cw, ch) = crate::render::cell_pixels(canvas.cols, canvas.rows);
    let pixel_x = (mouse.column as usize * cw * canvas.width / (canvas.cols as usize * cw).max(1))
        .min(canvas.width.saturating_sub(1));
    let row = mouse.row.checked_sub(HEADER_ROWS)? as usize;
    if row >= canvas.geom.rows as usize {
        return None;
    }
    let pixel_y = (row * ch * canvas.height / (canvas.geom.rows as usize * ch).max(1))
        .min(canvas.height.saturating_sub(1));
    for pane in &canvas.panes {
        if let Some(voxel) = voxel_at(editor, pane, pixel_x, pixel_y) {
            return Some((pane.axis, voxel));
        }
    }
    None
}
fn help(out: &mut impl Write, editor: &Editor) -> Result<()> {
    let (cols, _rows) = terminal::size()?;
    queue!(out, Clear(ClearType::All))?;
    let current_output = format!("Current output directory: {}", editor.out_dir.display());
    let help_lines = [
        "niv edit · spherical segmentation brush (raw voxel coordinates)",
        "",
        "1/2/3      Focus X/Y/Z pane       Tab          Cycle focused pane",
        "f          Fill focused pane     ↑↓ / wheel   Scroll its slice",
        "←→         Move crosshair         Shift+wheel  Faster slice scroll (4x)",
        "+/-        Zoom around crosshair  Ctrl+wheel   Zoom",
        "[/]        Brush radius (µm)    Alt+wheel    Brush radius",
        "Left drag  Paint 3D sphere       Right drag   Erase 3D sphere",
        "Shift+click Move crosshair only  Space        Paint/erase at crosshair",
        "p / e      Paint / erase mode   n / N       New Trim / Fill segment",
        "S          Next segmentation    4–9         Select segmentation",
        "i          Sample fill intensity from nearby tissue",
        "I          Type fill intensity for active Fill segment · w edit grayscale window",
        "u / U      Undo / redo          v           Toggle masked/raw reference",
        "m          Toggle brain mask    o / O       Reduce/increase segment opacity",
        "s          Save derived images + every segmentation to output directory",
        "q          Quit (confirmation if unsaved)   Ctrl+C  Force exit",
        "? / Esc    Return to editing",
        "",
        "Trim: set derived intensity=0 and brain mask=0 where painted.",
        "Fill: set derived intensity to segment fill value and brain mask=1.",
        "Original NIfTI files are never modified. Save to a separate directory.",
        "Reopen previously saved segments with --trim and --fill arguments.",
        current_output.as_str(),
    ];
    for (i, text) in help_lines.iter().enumerate() {
        queue!(
            out,
            MoveTo(0, i as u16),
            SetForegroundColor(if i == 0 { Color::Cyan } else { Color::Grey }),
            Print(cut(text, cols as usize)),
            ResetColor
        )?;
    }
    out.flush()?;
    Ok(())
}

fn paint_path(editor: &mut Editor, from: [usize; 3], to: [usize; 3], erase: bool) {
    let d = [
        to[0] as f32 - from[0] as f32,
        to[1] as f32 - from[1] as f32,
        to[2] as f32 - from[2] as f32,
    ];
    // Intermediate spheres prevent discontinuities during rapid mouse drags.
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let smallest_radius = editor.brush_um
        / (physical_scale(&editor.source)
            * editor
                .source
                .spacing
                .iter()
                .copied()
                .fold(f32::INFINITY, f32::min))
        .max(0.001);
    let steps = (distance / (smallest_radius * 0.5).max(1.0))
        .ceil()
        .clamp(1., 128.) as usize;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let c = std::array::from_fn(|k| {
            (from[k] as f32 + d[k] * t)
                .round()
                .clamp(0., editor.dims()[k].saturating_sub(1) as f32) as usize
        });
        editor.paint_sphere(c, erase);
    }
}

fn run(args: Args) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("niv edit requires an interactive Kitty-compatible terminal");
    }
    let mut editor = Editor::load(args)?;
    let _guard = crate::TerminalGuard::enter(true, false)?;
    let mut stdout = io::stdout().lock();
    execute!(stdout, Clear(ClearType::All))?;
    let mut active_image = None;
    let mut next_id = 5_000_000u32;
    let mut current_canvas = canvas(&editor.source, &editor)?;
    let mut redraw = true;
    let mut help_open = false;
    let mut last_frame = Instant::now() - Duration::from_millis(100);
    let frame_interval = Duration::from_millis(40);
    let mut dragging: Option<(MouseButton, [usize; 3])> = None;
    loop {
        if redraw && last_frame.elapsed() >= frame_interval {
            if !help_open {
                current_canvas = draw(&editor, &mut stdout, &mut active_image, &mut next_id)?;
            }
            redraw = false;
            last_frame = Instant::now();
        }
        if !event::poll(Duration::from_millis(15))? {
            continue;
        }
        let event = event::read()?;
        match event {
            Event::Resize(_, _) => {
                redraw = true;
            }
            Event::Key(key) => {
                if key.kind == event::KeyEventKind::Release {
                    continue;
                }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                if help_open {
                    if matches!(
                        key.code,
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
                    ) {
                        help_open = false;
                        redraw = true;
                    }
                    continue;
                }
                if editor.edit_value.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            editor.edit_value = None;
                        }
                        KeyCode::Backspace => {
                            if let Some((_, txt)) = editor.edit_value.as_mut() {
                                txt.pop();
                            }
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some((_, txt)) = editor.edit_value.as_mut() {
                                txt.clear();
                            }
                        }
                        KeyCode::Char(c)
                            if c.is_ascii_digit()
                                || matches!(c, '.' | ',' | '-' | 'e' | 'E' | '+') =>
                        {
                            if let Some((_, txt)) = editor.edit_value.as_mut() {
                                txt.push(c);
                            }
                        }
                        KeyCode::Enter => {
                            let (entry, string) = editor.edit_value.take().unwrap();
                            let answer = match entry {
                                EntryKind::Fill => match string.parse::<f32>() {
                                    Ok(v) if v.is_finite() && v >= 0. => {
                                        if editor.segments[editor.active_seg].role == Role::Fill {
                                            editor.segments[editor.active_seg].fill_value = v;
                                            editor.rebuild_all();
                                            editor.dirty = true;
                                            Ok(format!("Fill intensity set to {v}"))
                                        } else {
                                            Err("Choose a Fill segment first".to_owned())
                                        }
                                    }
                                    _ => Err("Invalid finite, nonnegative intensity".to_owned()),
                                },
                                EntryKind::Window => {
                                    match string.split_once(',').and_then(|(a, b)| {
                                        Some((
                                            a.trim().parse::<f32>().ok()?,
                                            b.trim().parse::<f32>().ok()?,
                                        ))
                                    }) {
                                        Some((lo, hi))
                                            if lo.is_finite() && hi.is_finite() && lo < hi =>
                                        {
                                            editor.windows = (lo, hi);
                                            Ok(format!("Display window {lo}..{hi}"))
                                        }
                                        _ => Err("Window must be two increasing numbers: min,max"
                                            .to_owned()),
                                    }
                                }
                            };
                            editor.notice = match answer {
                                Ok(msg) | Err(msg) => msg,
                            };
                        }
                        _ => {}
                    }
                    redraw = true;
                    continue;
                }
                match key.code {
                    KeyCode::Char('?') => {
                        editor.finish_stroke();
                        if let Some(id) = active_image.take() {
                            kitty::delete(&mut stdout, id)?;
                        }
                        help(&mut stdout, &editor)?;
                        help_open = true;
                    }
                    KeyCode::Char('q') => {
                        if editor.dirty && !editor.quit_armed {
                            editor.quit_armed = true;
                        } else {
                            break;
                        }
                    }
                    KeyCode::Esc => {
                        editor.quit_armed = false;
                        editor.notice.clear();
                    }
                    KeyCode::Char('1' | '2' | '3') => {
                        editor.active_view = match key.code {
                            KeyCode::Char('1') => 0,
                            KeyCode::Char('2') => 1,
                            _ => 2,
                        };
                        editor.hover = None;
                    }
                    KeyCode::Char('4'..='9') => {
                        let selected = match key.code {
                            KeyCode::Char(c) => c as usize - '4' as usize,
                            _ => 0,
                        };
                        if selected < editor.segments.len() {
                            editor.finish_stroke();
                            editor.active_seg = selected;
                        }
                    }
                    KeyCode::Tab => {
                        editor.active_view = (editor.active_view + 1) % 3;
                        editor.hover = None;
                    }
                    KeyCode::BackTab => {
                        editor.active_view = (editor.active_view + 2) % 3;
                        editor.hover = None;
                    }
                    KeyCode::Char('f') => {
                        editor.solo = !editor.solo;
                        editor.hover = None;
                    }
                    KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k') => {
                        let step = if key.modifiers.contains(KeyModifiers::SHIFT) {
                            4
                        } else if key.modifiers.contains(KeyModifiers::CONTROL) {
                            3
                        } else if key.modifiers.contains(KeyModifiers::ALT) {
                            2
                        } else {
                            1
                        };
                        let forward = matches!(key.code, KeyCode::Down | KeyCode::Char('j'));
                        editor.move_slice(editor.active_view, if forward { step } else { -step });
                        editor.hover = None;
                    }
                    KeyCode::Left | KeyCode::Right => {
                        let (ha, _) = axes(editor.active_view);
                        let step = if matches!(key.code, KeyCode::Right) {
                            1
                        } else {
                            -1
                        };
                        editor.move_slice(ha, step);
                    }
                    KeyCode::Char('[') => editor.radius(-25.),
                    KeyCode::Char(']') => editor.radius(25.),
                    KeyCode::Char('+' | '=') => editor.change_zoom(0.25),
                    KeyCode::Char('-') => editor.change_zoom(-0.25),
                    KeyCode::Char('0') => editor.zoom = 1.,
                    KeyCode::Char('p') => editor.tool = Tool::Paint,
                    KeyCode::Char('e') => editor.tool = Tool::Erase,
                    KeyCode::Char(' ') => {
                        editor.begin_stroke();
                        let coords = editor.coordinates;
                        let erase = editor.tool == Tool::Erase;
                        editor.paint_sphere(coords, erase);
                        editor.finish_stroke();
                    }
                    KeyCode::Char('n') => editor.new_segment(Role::Trim),
                    KeyCode::Char('N') => editor.new_segment(Role::Fill),
                    KeyCode::Char('S') => {
                        editor.finish_stroke();
                        editor.active_seg = (editor.active_seg + 1) % editor.segments.len();
                    }
                    KeyCode::Char('u') => editor.undo(),
                    KeyCode::Char('U') => editor.redo(),
                    KeyCode::Char('v') => {
                        if editor.raw.is_some() {
                            editor.show_raw = !editor.show_raw;
                        } else {
                            editor.notice = "No --raw volume loaded".into();
                        }
                    }
                    KeyCode::Char('m') => editor.show_mask = !editor.show_mask,
                    KeyCode::Char('o') => {
                        editor.overlay_opacity = (editor.overlay_opacity - 0.1).max(0.)
                    }
                    KeyCode::Char('O') => {
                        editor.overlay_opacity = (editor.overlay_opacity + 0.1).min(1.)
                    }
                    KeyCode::Char('i') => editor.sample_fill(),
                    KeyCode::Char('I') => {
                        if editor.segments[editor.active_seg].role == Role::Fill {
                            editor.edit_value = Some((
                                EntryKind::Fill,
                                format!("{}", editor.segments[editor.active_seg].fill_value),
                            ));
                        } else {
                            editor.notice =
                                "New Fill segment with N before setting its intensity".into();
                        }
                    }
                    KeyCode::Char('w') => {
                        editor.edit_value = Some((
                            EntryKind::Window,
                            format!("{},{}", editor.windows.0, editor.windows.1),
                        ))
                    }
                    KeyCode::Char('s') => {
                        if let Err(e) = editor.save() {
                            editor.notice = format!("Save error: {e:#}");
                        }
                    }
                    _ => {}
                }
                redraw = true;
            }
            Event::Mouse(mouse) if !help_open => {
                let hit = mouse_voxel(&editor, &current_canvas, mouse);
                match mouse.kind {
                    MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight => {
                        if let Some((axis, _)) = hit {
                            editor.active_view = axis;
                        }
                        let up = matches!(
                            mouse.kind,
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft
                        );
                        let sign = if up { -1. } else { 1. };
                        if mouse.modifiers.contains(KeyModifiers::ALT) {
                            editor.radius(sign * 25.);
                        } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                            editor.change_zoom(-sign * 0.25);
                        } else {
                            let step = if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                                4
                            } else {
                                1
                            };
                            editor.move_slice(
                                editor.active_view,
                                if up { -(step as isize) } else { step as isize },
                            );
                        }
                        redraw = true;
                    }
                    MouseEventKind::Moved => {
                        if let Some((axis, p)) = hit {
                            editor.active_view = axis;
                            editor.hover = Some(p);
                            redraw = true;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => {
                        if let Some((axis, p)) = hit {
                            editor.active_view = axis;
                            editor.hover = Some(p);
                            editor.coordinates = p;
                            if !mouse.modifiers.contains(KeyModifiers::SHIFT) {
                                editor.begin_stroke();
                                let erase = mouse.kind == MouseEventKind::Down(MouseButton::Right);
                                editor.paint_sphere(p, erase);
                                dragging = Some((
                                    if erase {
                                        MouseButton::Right
                                    } else {
                                        MouseButton::Left
                                    },
                                    p,
                                ));
                            }
                            redraw = true;
                        }
                    }
                    MouseEventKind::Drag(button) => {
                        if let (Some((axis, p)), Some((started, last))) = (hit, dragging) {
                            if button == started {
                                editor.active_view = axis;
                                editor.hover = Some(p);
                                editor.coordinates = p;
                                paint_path(&mut editor, last, p, button == MouseButton::Right);
                                dragging = Some((button, p));
                                redraw = true;
                            }
                        }
                    }
                    MouseEventKind::Up(_) => {
                        editor.finish_stroke();
                        dragging = None;
                        redraw = true;
                    }
                    MouseEventKind::Down(MouseButton::Middle) => {
                        if let Some((axis, p)) = hit {
                            editor.active_view = axis;
                            editor.coordinates = p;
                            editor.hover = Some(p);
                            redraw = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(id) = active_image {
        kitty::delete(&mut stdout, id)?;
    }
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn synthetic(dims: [usize; 3], spacing: [f32; 3]) -> Editor {
        let data = Array3::from_elem((dims[0], dims[1], dims[2]), 10.);
        let mask = Array3::from_elem((dims[0], dims[1], dims[2]), 1.);
        let vol = Volume {
            data,
            spacing,
            spatial_units: 3,
            sform: None,
            datatype: 16,
        };
        let maskvol = Volume {
            data: mask,
            spacing,
            spatial_units: 3,
            sform: None,
            datatype: 2,
        };
        let mut e = Editor {
            coordinates: dims.map(|x| x / 2),
            brush_offsets: sphere_offsets(spacing, 1., 50.),
            brush_um: 50.,
            work: vol.data.clone(),
            binary: Array3::from_elem((dims[0], dims[1], dims[2]), 1),
            source: vol,
            source_path: PathBuf::from("original.nii.gz"),
            mask_source: maskvol,
            mask_path: PathBuf::from("mask.nii.gz"),
            raw: None,
            segments: Vec::new(),
            active_seg: 0,
            active_view: 0,
            solo: false,
            zoom: 1.,
            tool: Tool::Paint,
            show_raw: false,
            show_mask: true,
            overlay_opacity: 0.45,
            windows: (0., 100.),
            out_dir: PathBuf::from("out"),
            overwrite: false,
            saved_once: false,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: HashMap::new(),
            hover: None,
            dirty: false,
            notice: String::new(),
            quit_armed: false,
            edit_value: None,
        };
        e.new_segment(Role::Trim);
        e
    }
    #[test]
    fn sphere_respects_physical_spacing() {
        let offsets = sphere_offsets([50., 50., 100.], 1., 100.);
        assert!(offsets.contains(&[2, 0, 0]));
        assert!(offsets.contains(&[0, 0, 1]));
        assert!(!offsets.contains(&[0, 0, 2]));
    }
    #[test]
    fn trim_then_fill_updates_both_outputs_and_preserves_original() {
        let mut e = synthetic([9, 9, 9], [50., 50., 50.]);
        let mid = e.coordinates;
        e.begin_stroke();
        e.paint_sphere(mid, false);
        e.finish_stroke();
        assert_eq!(e.work[mid], 0.);
        assert_eq!(e.binary[mid], 0);
        assert_eq!(e.source.data[mid], 10.);
        e.new_segment(Role::Fill);
        e.segments[e.active_seg].fill_value = 37.;
        e.begin_stroke();
        e.paint_sphere(mid, false);
        e.finish_stroke();
        assert_eq!(e.work[mid], 37.);
        assert_eq!(e.binary[mid], 1);
        e.undo();
        assert_eq!(e.work[mid], 0.);
        e.redo();
        assert_eq!(e.work[mid], 37.);
    }
    #[test]
    fn saved_segment_metadata_preserves_fill_intensity_and_color() {
        let metadata = SegmentMetadata {
            version: 1,
            role: "fill".into(),
            fill_value: 37.125,
            color: [24, 210, 190],
        };
        let serialized = toml::to_string_pretty(&metadata).unwrap();
        let restored: SegmentMetadata = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.fill_value, 37.125);
        assert_eq!(restored.color, [24, 210, 190]);
        assert_eq!(restored.role, "fill");
    }
    #[test]
    fn completed_drag_stroke_is_a_single_undo_operation() {
        let mut e = synthetic([20, 20, 20], [50.; 3]);
        e.begin_stroke();
        paint_path(&mut e, [3, 9, 9], [15, 9, 9], false);
        e.finish_stroke();
        assert_eq!(e.undo.len(), 1);
        assert_eq!(e.work[[10, 9, 9]], 0.);
        e.undo();
        assert_eq!(e.work[[10, 9, 9]], 10.);
    }
    #[test]
    fn all_three_planes_share_same_crosshair_voxel() {
        let e = synthetic([11, 12, 13], [50.; 3]);
        for axis in 0..3 {
            let pane = Pane {
                axis,
                x: 0,
                y: 0,
                w: 110,
                h: 120,
            };
            let (x, y) = pixel_for(&e, &pane, e.coordinates);
            let found = voxel_at(&e, &pane, x as usize, y as usize).unwrap();
            let (h, v) = axes(axis);
            assert!((found[h] as isize - e.coordinates[h] as isize).abs() <= 1);
            assert!((found[v] as isize - e.coordinates[v] as isize).abs() <= 1);
        }
    }
    #[test]
    fn erase_and_undo_are_non_destructive() {
        let mut e = synthetic([7, 7, 7], [50.; 3]);
        let middle = e.coordinates;
        e.begin_stroke();
        e.paint_sphere(middle, false);
        e.finish_stroke();
        e.begin_stroke();
        e.paint_sphere(middle, true);
        e.finish_stroke();
        assert_eq!(e.segments[0].voxels[middle], 0);
        e.undo();
        assert_eq!(e.segments[0].voxels[middle], 1);
    }
}
