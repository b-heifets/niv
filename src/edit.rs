//! Experimental, non-destructive three-plane NIfTI segmentation editor.
//! `niv IMAGE` then Ctrl+E, or `niv edit IMAGE [--mask MASK] [--out DIR]`.
//! All coordinates are original voxel indices; this mode never reorients data.
use crate::{kitty, path_complete, render::Geometry, volume::Volume};
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
const FOOTER_ROWS: u16 = 5;
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
    Minimum,
    Maximum,
    Name,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ApplyRegion {
    Off,
    Inside,
    Outside,
}
impl ApplyRegion {
    fn next(self) -> Self {
        match self {
            Self::Off => Self::Inside,
            Self::Inside => Self::Outside,
            Self::Outside => Self::Off,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Inside => "inside",
            Self::Outside => "outside",
        }
    }
}
#[derive(Clone)]
struct Segment {
    name: String,
    color: [u8; 3],
    voxels: Array3<u8>,
    fill_value: f32,
    visible: bool,
    region: ApplyRegion,
}
struct ImageLayer {
    path: PathBuf,
    volume: Volume,
    window: (f32, f32),
}
struct PickerState {
    text: String,
    matches: path_complete::PathMatches,
}
impl PickerState {
    fn open() -> Self {
        let text = String::new();
        Self {
            matches: path_complete::search(&text),
            text,
        }
    }
    fn refresh(&mut self) {
        self.matches = path_complete::search(&self.text);
    }
}
#[derive(Serialize, Deserialize)]
struct SegmentMetadata {
    version: u32,
    role: String,
    fill_value: f32,
    color: [u8; 3],
    #[serde(default = "default_true")]
    visible: bool,
    #[serde(default)]
    region: Option<ApplyRegion>,
}
fn default_true() -> bool {
    true
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
    mask_path: Option<PathBuf>,
    export_mask: bool,
    raw: Option<Volume>,
    extra_images: Vec<ImageLayer>,
    selected_image: usize,
    picker: Option<PickerState>,
    info_open: bool,
    preview_output: bool,
    work: Array3<f32>,
    binary: Array3<u8>,
    segments: Vec<Segment>,
    active_seg: usize,
    coordinates: [usize; 3],
    active_view: usize,
    visible_views: [bool; 3],
    zoom: [f32; 3],
    centers: [[f64; 2]; 3],
    rotation: [u8; 3],
    flip_h: [bool; 3],
    flip_v: [bool; 3],
    stride: usize,
    brush_um: f32,
    brush_offsets: Vec<[isize; 3]>,
    tool: Tool,
    show_mask: bool,
    overlay_opacity: f32,
    seg_list_open: bool,
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
    mask: Option<PathBuf>,
    raw: Option<PathBuf>,
    out: Option<PathBuf>,
    trims: Vec<PathBuf>,
    fills: Vec<PathBuf>,
    generic: Vec<PathBuf>,
    radius_um: f32,
    overwrite: bool,
    binarize_mask: bool,
}

fn edit_usage() -> &'static str {
    r#"niv edit — NIfTI spherical segmentation editor


Usage: niv image.nii.gz   (Ctrl+E to edit an already loaded image)
       niv edit IMAGE.nii.gz [--mask brain_mask.nii.gz] [--raw image.nii.gz]
       niv edit --masked IMAGE.nii.gz [--mask brain_mask.nii.gz]
               [--binarize-mask] [--out DIR] [--trim SEG.nii.gz]...
               [--fill SEG.nii.gz]... [--segment SEG.nii.gz]...
               [--radius-um 150] [--overwrite]

A brain mask is optional. With no mask, the image is the input and a brain
mask can be derived on save by toggling B or specifying --binarize-mask.
No segments are created automatically: press a to add a segmentation.
Original files are never overwritten. Press ? in the editor for help.
"#
}
fn parse_args(args: &[String]) -> Result<Option<Args>> {
    let mut masked = None;
    let mut mask = None;
    let mut raw = None;
    let mut out = None;
    let mut trims = Vec::new();
    let mut fills = Vec::new();
    let mut generic = Vec::new();
    let mut radius_um = 150f32;
    let mut overwrite = false;
    let mut binarize_mask = false;
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
        if opt == "--binarize-mask" {
            binarize_mask = true;
            i += 1;
            continue;
        }
        if !opt.starts_with('-') {
            if masked.is_some() {
                bail!("Only one input image is supported");
            }
            masked = Some(PathBuf::from(opt));
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
            "--segment" => generic.push(PathBuf::from(next)),
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
        masked: masked.context("Missing input NIfTI image")?,
        mask,
        raw,
        out,
        trims,
        fills,
        generic,
        radius_um,
        overwrite,
        binarize_mask,
    }))
}

pub fn run_from_viewer(
    path: &Path,
    axis: usize,
    slice: usize,
    window: (f32, f32),
    transforms: [(u16, bool, bool); 3],
    out: &mut impl Write,
) -> Result<()> {
    let args = Args {
        masked: path.to_path_buf(),
        mask: None,
        raw: None,
        out: None,
        trims: Vec::new(),
        fills: Vec::new(),
        generic: Vec::new(),
        radius_um: 150.,
        overwrite: false,
        binarize_mask: false,
    };
    let mut editor = Editor::load(args)?;
    if window.0.is_finite() && window.1.is_finite() && window.0 < window.1 {
        editor.windows = window;
    }
    for (a, (rotation, h, v)) in transforms.into_iter().enumerate() {
        editor.rotation[a] = ((rotation / 90) % 4) as u8;
        editor.flip_h[a] = h;
        editor.flip_v[a] = v;
    }
    editor.active_view = axis.min(2);
    let focused = editor.active_view;
    editor.coordinates[focused] = slice.min(editor.dims()[focused].saturating_sub(1));
    run_loop(editor, out)
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
    let trim = [[80, 218, 124], [92, 202, 150], [158, 222, 93]];
    let fill = [
        [80, 218, 124],
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
        let mask_source = if let Some(path) = args.mask.as_ref() {
            let mask = Volume::load(path)?;
            source
                .validate_grid(&mask)
                .context("Brain mask grid does not match the input image")?;
            mask
        } else {
            // Derive a binary reference from positive, finite input intensities.
            Volume {
                data: source
                    .data
                    .mapv(|v| if v.is_finite() && v > 0. { 1. } else { 0. }),
                spacing: source.spacing,
                spatial_units: source.spatial_units,
                sform: source.sform,
                datatype: 2,
            }
        };
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
            mask_path: args.mask.clone(),
            export_mask: args.mask.is_some() || args.binarize_mask,
            raw,
            extra_images: Vec::new(),
            selected_image: 0,
            picker: None,
            info_open: false,
            preview_output: false,
            work,
            binary,
            segments: Vec::new(),
            active_seg: 0,
            active_view: 0,
            visible_views: [true; 3],
            zoom: [1.; 3],
            centers: [[0.; 2]; 3],
            rotation: [0; 3],
            flip_h: [false; 3],
            flip_v: [false; 3],
            stride: 1,
            tool: Tool::Paint,
            show_mask: args.mask.is_some(),
            overlay_opacity: 0.22,
            seg_list_open: false,
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
            editor.load_segment(Role::Trim, &path, false)?;
        }
        for path in args.fills {
            editor.load_segment(Role::Fill, &path, false)?;
        }
        for path in args.generic {
            editor.load_segment(Role::Fill, &path, true)?;
        }
        if let (Some(path), Some(vol)) = (args.raw.as_ref(), editor.raw.take()) {
            let window = vol.auto_window();
            editor.extra_images.push(ImageLayer {
                path: path.clone(),
                volume: vol,
                window,
            });
        }
        for axis in 0..3 {
            let (h, v) = axes(axis);
            editor.centers[axis] = [(dims[h] - 1) as f64 / 2., (dims[v] - 1) as f64 / 2.];
        }
        editor.active_seg = editor.segments.len().saturating_sub(1);
        editor.rebuild_all();
        if editor.source.micrometre_scale().is_none() {
            editor.notice =
                "NIfTI spatial units unspecified; assuming voxel spacing is in mm".into();
        }
        Ok(editor)
    }
    fn load_segment(&mut self, role: Role, path: &Path, generic: bool) -> Result<()> {
        let vol = Volume::load(path)?;
        self.source.validate_grid(&vol)?;
        let count = self.segments.len();
        let metadata = if metadata_path(path).is_file() {
            let metadata: SegmentMetadata =
                toml::from_str(&fs::read_to_string(metadata_path(path))?)?;
            if metadata.version != 1
                || (metadata.role.to_lowercase() != role.name().to_lowercase()
                    && metadata.role.to_lowercase() != "segment")
            {
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
        let visible = metadata.as_ref().map(|m| m.visible).unwrap_or(true);
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
        let region = metadata
            .as_ref()
            .and_then(|m| m.region)
            .unwrap_or(if generic {
                ApplyRegion::Off
            } else {
                ApplyRegion::Inside
            }); // legacy --trim/--fill semantics
        self.segments.push(Segment {
            name: stem,
            color,
            voxels: vol.data.mapv(|v| u8::from(v.is_finite() && v > 0.5)),
            fill_value: if generic || (role == Role::Trim && metadata.is_none()) {
                0.
            } else {
                fill_value
            },
            visible,
            region,
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
    fn new_segment(&mut self) {
        self.finish_stroke();
        let count = self.segments.len() + 1;
        let shape = self.source.data.raw_dim();
        self.segments.push(Segment {
            name: format!("seg_{count:02}"),
            color: palette(Role::Fill, count - 1),
            voxels: Array3::zeros(shape),
            fill_value: 0.,
            visible: true,
            region: ApplyRegion::Off,
        });
        self.active_seg = self.segments.len() - 1;
        self.dirty = true;
        self.notice = format!(
            "New segmentation · {} · press t to enable intensity application",
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
            let selected = seg.voxels[pos] > 0;
            let applies = match seg.region {
                ApplyRegion::Off => false,
                ApplyRegion::Inside => selected,
                ApplyRegion::Outside => !selected,
            };
            if applies {
                value = seg.fill_value;
                inside = u8::from(value.is_finite() && value > 0.);
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
        if self.segments.is_empty() {
            self.notice = "Create a segmentation with a first".into();
            return 0;
        }
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
    fn add_image(&mut self, path: &Path) -> Result<()> {
        if self.source_path == path || self.extra_images.iter().any(|v| v.path == path) {
            self.notice = "This image is already loaded".into();
            return Ok(());
        }
        let volume = Volume::load(path)?;
        self.source
            .validate_grid(&volume)
            .context("Additional image must match source voxel grid")?;
        let window = volume.auto_window();
        self.extra_images.push(ImageLayer {
            path: path.to_path_buf(),
            volume,
            window,
        });
        self.selected_image = self.extra_images.len();
        self.notice = format!("Loaded image: {}", name(path));
        Ok(())
    }
    fn selected_window(&self) -> (f32, f32) {
        if self.selected_image == 0 {
            self.windows
        } else {
            self.extra_images[self.selected_image - 1].window
        }
    }
    fn set_selected_window(&mut self, window: (f32, f32)) {
        if self.selected_image == 0 {
            self.windows = window;
        } else {
            self.extra_images[self.selected_image - 1].window = window;
        }
    }
    fn selected_path(&self) -> &Path {
        if self.selected_image == 0 {
            &self.source_path
        } else {
            &self.extra_images[self.selected_image - 1].path
        }
    }
    fn selected_volume(&self) -> &Volume {
        if self.selected_image == 0 {
            &self.source
        } else {
            &self.extra_images[self.selected_image - 1].volume
        }
    }
    fn cycle_image(&mut self) {
        self.selected_image = (self.selected_image + 1) % (self.extra_images.len() + 1);
    }
    fn cycle_apply_region(&mut self) {
        self.finish_stroke();
        if let Some(seg) = self.segments.get_mut(self.active_seg) {
            seg.region = seg.region.next();
            let label = seg.region.label();
            self.rebuild_all();
            self.dirty = true;
            self.notice = format!("Intensity operation: {label} (press I to set target value)");
        }
    }
    fn change_zoom(&mut self, delta: f32) {
        let axis = self.active_view;
        if self.zoom[axis] <= 1.001 && delta > 0. {
            let (h, v) = axes(axis);
            self.centers[axis] = [self.coordinates[h] as f64, self.coordinates[v] as f64];
        }
        self.zoom[axis] = (self.zoom[axis] + delta).clamp(1., 16.);
    }
    fn pan(&mut self, axis: usize, dh: f64, dv: f64) {
        let (h, v) = axes(axis);
        self.centers[axis][0] = (self.centers[axis][0] + dh).clamp(0., (self.dims()[h] - 1) as f64);
        self.centers[axis][1] = (self.centers[axis][1] + dv).clamp(0., (self.dims()[v] - 1) as f64);
    }
    fn move_crosshair_in_plane(&mut self, axis: usize, p: [usize; 3]) {
        let (h, v) = axes(axis);
        self.coordinates[h] = p[h];
        self.coordinates[v] = p[v];
    }
    fn toggle_view(&mut self, axis: usize) {
        if !self.visible_views[axis] {
            self.visible_views[axis] = true;
            self.active_view = axis;
        } else if self.visible_views.iter().filter(|&&v| v).count() > 1 {
            self.visible_views[axis] = false;
            if self.active_view == axis {
                self.active_view = self.visible_views.iter().position(|&v| v).unwrap_or(0);
            }
        } else {
            self.notice = "At least one orthogonal view must remain visible".into();
        }
        self.hover = None;
    }
    fn seg_visible(&mut self) {
        if let Some(seg) = self.segments.get_mut(self.active_seg) {
            seg.visible = !seg.visible;
        }
    }
    fn sanitize_name(input: &str) -> String {
        input
            .trim()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim_matches('_')
            .to_string()
    }
    fn seg_name(&mut self, text: &str) {
        let clean = Self::sanitize_name(text);
        if clean.is_empty() {
            self.notice = "Segment name cannot be empty".into();
            return;
        }
        let existing = self
            .segments
            .iter()
            .enumerate()
            .any(|(i, s)| i != self.active_seg && s.name == clean);
        if existing {
            self.notice = "Segment names must be unique".into();
            return;
        }
        if let Some(seg) = self.segments.get_mut(self.active_seg) {
            seg.name = clean;
            self.dirty = true;
        }
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
        let source = self.selected_volume().data[p];
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
                            let v = self.selected_volume().data
                                [[a[0] as usize, a[1] as usize, a[2] as usize]];
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
        if !self.segments.is_empty() {
            self.segments[self.active_seg].fill_value = value;
            self.rebuild_all();
            self.dirty = true;
            self.notice =
                format!("Segmentation target intensity = {value:.5} (sampled near cursor)");
        } else {
            self.notice = "Create a segmentation with a first".into();
        }
    }
    fn output_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self
            .out_dir
            .join(format!("{}_niv_edited.nii.gz", name(&self.source_path)))];
        if self.export_mask {
            let filename = if let Some(path) = self.mask_path.as_ref() {
                format!("{}_niv_edited.nii.gz", name(path))
            } else {
                format!("{}_niv_brain_mask.nii.gz", name(&self.source_path))
            };
            paths.push(self.out_dir.join(filename));
        }
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
        let segment_start = if self.export_mask { 2 } else { 1 };
        // Write to temporary NIfTI files and rename. Existing inputs are never overwritten.
        for (i, path) in paths.iter().enumerate() {
            let tmp = self.out_dir.join(format!(".niv_{}.part.nii.gz", i));
            let result = (|| -> Result<()> {
                if i == 0 {
                    WriterOptions::new(&tmp)
                        .reference_file(&self.source_path)
                        .write_nifti(&self.work)?;
                } else if self.export_mask && i == 1 {
                    let ref_path = self.mask_path.as_ref().unwrap_or(&self.source_path);
                    WriterOptions::new(&tmp)
                        .reference_file(ref_path)
                        .write_nifti(&self.binary)?;
                } else {
                    WriterOptions::new(&tmp)
                        .reference_file(&self.source_path)
                        .write_nifti(&self.segments[i - segment_start].voxels)?;
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            result.with_context(|| format!("Writing {}", path.display()))?;
            fs::rename(&tmp, path).with_context(|| format!("Saving {}", path.display()))?;
            if i >= segment_start {
                let seg = &self.segments[i - segment_start];
                let info = SegmentMetadata {
                    version: 1,
                    role: "segment".into(),
                    fill_value: seg.fill_value,
                    color: seg.color,
                    visible: seg.visible,
                    region: Some(seg.region),
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
fn source_aspect(vol: &Volume, axis: usize, rotation: u8) -> f64 {
    let (h, v) = axes(axis);
    let ratio = vol.dims()[h] as f64 * vol.spacing[h] as f64
        / (vol.dims()[v] as f64 * vol.spacing[v] as f64);
    if rotation % 2 == 1 {
        1. / ratio
    } else {
        ratio
    }
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
    let tiles = (0..3)
        .filter(|&a| editor.visible_views[a])
        .collect::<Vec<_>>();
    let margin = 6usize.min(width / 20).min(height / 20);
    for (n, &axis) in tiles.iter().enumerate() {
        let col_start = n * width / tiles.len();
        let col_end = (n + 1) * width / tiles.len();
        let available_w = col_end.saturating_sub(col_start + 2 * margin).max(1);
        let available_h = height.saturating_sub(2 * margin).max(1);
        let ratio = source_aspect(vol, axis, editor.rotation[axis]).clamp(0.01, 100.);
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
    (editor.centers[axis][0], editor.centers[axis][1])
}
// Screen coordinates have right/up positive directions. Orthoplane rotation
// and flipping are display-only: the voxel array is never reoriented.
fn display_from_native(editor: &Editor, axis: usize, uv: [f64; 2]) -> [f64; 2] {
    let (u, v) = (uv[0], uv[1]);
    let (mut x, mut y) = match editor.rotation[axis] % 4 {
        0 => (u, v),
        1 => (v, -u),
        2 => (-u, -v),
        _ => (-v, u),
    };
    if editor.flip_h[axis] {
        x = -x;
    }
    if editor.flip_v[axis] {
        y = -y;
    }
    [x, y]
}
fn native_from_display(editor: &Editor, axis: usize, xy: [f64; 2]) -> [f64; 2] {
    let mut x = xy[0];
    let mut y = xy[1];
    if editor.flip_h[axis] {
        x = -x;
    }
    if editor.flip_v[axis] {
        y = -y;
    }
    let (u, v) = match editor.rotation[axis] % 4 {
        0 => (x, y),
        1 => (-y, x),
        2 => (-x, -y),
        _ => (y, -x),
    };
    [u, v]
}
fn voxel_at(editor: &Editor, pane: &Pane, x: usize, y: usize) -> Option<[usize; 3]> {
    if x < pane.x || x >= pane.x + pane.w || y < pane.y || y >= pane.y + pane.h {
        return None;
    }
    let (ha, va) = axes(pane.axis);
    let dims = editor.dims();
    let [u, v] = native_from_display(
        editor,
        pane.axis,
        [
            ((x - pane.x) as f64 + 0.5) / pane.w as f64 - 0.5,
            0.5 - ((y - pane.y) as f64 + 0.5) / pane.h as f64,
        ],
    );
    let (center_h, center_v) = plane_center(editor, pane.axis);
    let h = (center_h + u * dims[ha] as f64 / editor.zoom[pane.axis] as f64).round() as isize;
    let v = (center_v + v * dims[va] as f64 / editor.zoom[pane.axis] as f64).round() as isize;
    if h < 0 || h >= dims[ha] as isize || v < 0 || v >= dims[va] as isize {
        return None;
    }
    let mut xyz = editor.coordinates;
    xyz[ha] = h as usize;
    xyz[va] = v as usize;
    Some(xyz)
}
fn pixel_for_native(editor: &Editor, pane: &Pane, h: f64, v: f64) -> (isize, isize) {
    let (ha, va) = axes(pane.axis);
    let dims = editor.dims();
    let (ch, cv) = plane_center(editor, pane.axis);
    let [x, y] = display_from_native(
        editor,
        pane.axis,
        [
            (h - ch) / (dims[ha] as f64 / editor.zoom[pane.axis] as f64),
            (v - cv) / (dims[va] as f64 / editor.zoom[pane.axis] as f64),
        ],
    );
    (
        (pane.x as f64 + pane.w as f64 * (x + 0.5)).round() as isize,
        (pane.y as f64 + pane.h as f64 * (0.5 - y)).round() as isize,
    )
}
fn pixel_for(editor: &Editor, pane: &Pane, point: [usize; 3]) -> (isize, isize) {
    let (h, v) = axes(pane.axis);
    pixel_for_native(editor, pane, point[h] as f64, point[v] as f64)
}
fn pan_screen(editor: &mut Editor, pane: &Pane, dx: f64, dy: f64) {
    // Positive dx means the image moves right: shift the view center left.
    let native = native_from_display(editor, pane.axis, [-dx / pane.w as f64, dy / pane.h as f64]);
    let (h, v) = axes(pane.axis);
    editor.pan(
        pane.axis,
        native[0] * editor.dims()[h] as f64 / editor.zoom[pane.axis] as f64,
        native[1] * editor.dims()[v] as f64 / editor.zoom[pane.axis] as f64,
    );
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
    // Always show *original* data under translucent segments by default.
    // Composited/zeroed intensities appear only when preview_output is enabled.
    let value = if editor.selected_image == 0 {
        if editor.preview_output {
            editor.work[pos]
        } else {
            editor.source.data[pos]
        }
    } else {
        editor.extra_images[editor.selected_image - 1].volume.data[pos]
    };
    let (lo, hi) = editor.selected_window();
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
        if seg.visible && seg.voxels[pos] > 0 {
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
        // Draw the brush outline in every orthogonal view, not just the
        // hovered one. Use the actual 3D sphere/plane intersection.
        {
            let brush_color = editor
                .segments
                .get(editor.active_seg)
                .map(|seg| seg.color)
                .unwrap_or(palette(Role::Fill, 0));
            let brush_center = editor.hover.unwrap_or(editor.coordinates);
            let depth_axis = pane.axis;
            let d_um = ((brush_center[depth_axis] as isize
                - editor.coordinates[depth_axis] as isize)
                .abs() as f32)
                * editor.source.spacing[depth_axis]
                * physical_scale(&editor.source);
            let ghost = d_um > editor.brush_um;
            let planar_radius = if ghost {
                editor.brush_um
            } else {
                (editor.brush_um.powi(2) - d_um.powi(2)).sqrt()
            };
            let (ha, va) = axes(pane.axis);
            let su = (editor.source.spacing[ha] * physical_scale(&editor.source)).max(0.001);
            let sv = (editor.source.spacing[va] * physical_scale(&editor.source)).max(0.001);
            let n = 96usize;
            for j in 0..n {
                if ghost && j % 4 >= 2 {
                    continue;
                }
                let t = j as f64 * std::f64::consts::TAU / n as f64;
                let px = brush_center[ha] as f64 + t.cos() * planar_radius as f64 / su as f64;
                let py = brush_center[va] as f64 + t.sin() * planar_radius as f64 / sv as f64;
                let (x, y) = pixel_for_native(editor, pane, px, py);
                if x > pane.x as isize
                    && y > pane.y as isize
                    && x < (pane.x + pane.w - 1) as isize
                    && y < (pane.y + pane.h - 1) as isize
                {
                    let c = if ghost {
                        [brush_color[0] / 2, brush_color[1] / 2, brush_color[2] / 2]
                    } else {
                        brush_color
                    };
                    put(&mut rgb, canvas.width, canvas.height, x, y, c);
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
    let seg = editor.segments.get(editor.active_seg);
    let hover = editor.hover.unwrap_or(editor.coordinates);
    let v = editor.selected_volume().data[hover];
    let (col0, col1) = (
        Color::Rgb {
            r: 110,
            g: 220,
            b: 232,
        },
        Color::Grey,
    );
    queue!(
        out,
        MoveTo(0, 0),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(col0),
        Print(cut(
            &format!(
                "niv EDIT  [{},{},{}]  ·  focus {}  ·  voxel {:.5}",
                editor.coordinates[0],
                editor.coordinates[1],
                editor.coordinates[2],
                ["X", "Y", "Z"][editor.active_view],
                v
            ),
            canvas.cols as usize
        )),
        ResetColor
    )?;
    let seg_name = seg
        .map(|s| format!("{} · {} {:.4}", s.name, s.region.label(), s.fill_value))
        .unwrap_or_else(|| "no segmentation".into());
    let seg_color = seg
        .map(|s| Color::Rgb {
            r: s.color[0],
            g: s.color[1],
            b: s.color[2],
        })
        .unwrap_or(Color::DarkGrey);
    queue!(
        out,
        MoveTo(0, 1),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(col1),
        Print(format!(
            "brush {:.0}µm  ·  stride {}  ·  zoom {:.2}×  ·  ",
            editor.brush_um * 2.,
            editor.stride,
            editor.zoom[editor.active_view]
        )),
        SetForegroundColor(seg_color),
        Print(seg_name),
        ResetColor
    )?;
    queue!(out, MoveTo(0, HEADER_ROWS))?;
    let id = *next;
    *next = next.wrapping_add(1).max(1);
    kitty::replace(out, id, &frame, canvas.geom, 0, *active)?;
    *active = Some(id);
    let foot = HEADER_ROWS + canvas.geom.rows;
    let cols = canvas.cols as usize;
    queue!(
        out,
        MoveTo(0, foot),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Grey),
        Print("Segments ")
    )?;
    let mut seg_space = cols.saturating_sub(10);
    for (i, s) in editor.segments.iter().enumerate() {
        if seg_space == 0 {
            break;
        }
        let label = format!(
            "{}{}:{}{}  ",
            if i == editor.active_seg { "❯" } else { "" },
            i + 1,
            s.name,
            if s.visible { "" } else { " (off)" }
        );
        let clipped = cut(&label, seg_space);
        seg_space = seg_space.saturating_sub(clipped.chars().count());
        queue!(
            out,
            SetForegroundColor(Color::Rgb {
                r: s.color[0],
                g: s.color[1],
                b: s.color[2]
            }),
            Print(clipped)
        )?;
    }
    if editor.segments.is_empty() {
        queue!(out, Print("none · a to add"))?;
    }
    let top = format!("╭{}╮", "─".repeat(cols.saturating_sub(2)));
    let bottom = format!("╰{}╯", "─".repeat(cols.saturating_sub(2)));
    queue!(
        out,
        ResetColor,
        MoveTo(0, foot + 1),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(cut(&top, cols)),
        MoveTo(0, foot + 2),
        Clear(ClearType::CurrentLine),
        Print("│ ")
    )?;
    let mut img_space = cols.saturating_sub(4);
    for i in 0..=editor.extra_images.len() {
        if img_space == 0 {
            break;
        }
        let path = if i == 0 {
            &editor.source_path
        } else {
            &editor.extra_images[i - 1].path
        };
        let label = format!(
            "{}{}:{}  ",
            if i == editor.selected_image {
                "❯"
            } else {
                ""
            },
            i + 1,
            name(path)
        );
        let clipped = cut(&label, img_space);
        img_space = img_space.saturating_sub(clipped.chars().count());
        queue!(
            out,
            SetForegroundColor(if i == editor.selected_image {
                Color::Cyan
            } else {
                Color::Grey
            }),
            Print(clipped)
        )?;
    }
    queue!(
        out,
        ResetColor,
        MoveTo(0, foot + 3),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(cut(&bottom, cols)),
        ResetColor
    )?;
    let state = if editor.preview_output && editor.selected_image == 0 {
        "EDITED PREVIEW"
    } else {
        "ORIGINAL VIEW"
    };
    let msg = if let Some((kind, txt)) = editor.edit_value.as_ref() {
        format!(
            "{}: {txt}  ·  Enter apply / Esc cancel",
            match kind {
                EntryKind::Fill => "Intensity",
                EntryKind::Window => "Range min,max",
                EntryKind::Minimum => "Minimum",
                EntryKind::Maximum => "Maximum",
                EntryKind::Name => "Segment name",
            }
        )
    } else if editor.quit_armed {
        "Unsaved changes · q again to quit, s to save".into()
    } else if !editor.notice.is_empty() {
        editor.notice.clone()
    } else {
        format!(
            "{state} · {} · ? help",
            if editor.dirty {
                "UNSAVED"
            } else {
                "saved / unchanged"
            }
        )
    };
    queue!(
        out,
        MoveTo(0, foot + 4),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print(cut(&msg, cols)),
        ResetColor
    )?;
    out.flush()?;
    Ok(canvas)
}
// Clickable footer entries use the same label geometry as the bottom image bar.
fn footer_image_at(editor: &Editor, canvas: &Canvas, col: usize) -> Option<usize> {
    let mut cursor = 2usize;
    for i in 0..=editor.extra_images.len() {
        let path = if i == 0 {
            &editor.source_path
        } else {
            &editor.extra_images[i - 1].path
        };
        let text = format!(
            "{}{}:{}  ",
            if i == editor.selected_image {
                "❯"
            } else {
                ""
            },
            i + 1,
            name(path)
        );
        cursor += text.chars().count();
        if col < cursor {
            return Some(i);
        }
    }
    None
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
fn show_text_menu(out: &mut impl Write, title: &str, lines: &[String]) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let width = (lines.iter().map(|s| s.chars().count()).max().unwrap_or(40) + 6)
        .min(cols.saturating_sub(2) as usize)
        .max(24);
    let height = (lines.len() + 5)
        .min(rows.saturating_sub(2) as usize)
        .max(5);
    let x = (cols as usize).saturating_sub(width) / 2;
    let y = (rows as usize).saturating_sub(height) / 2;
    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    for i in 0..height {
        let (left, right) = if i == 0 {
            ('╭', '╮')
        } else if i == height - 1 {
            ('╰', '╯')
        } else {
            ('│', '│')
        };
        let filler = if i == 0 || i == height - 1 {
            '─'
        } else {
            ' '
        };
        let frame = format!("{left}{}{right}", filler.to_string().repeat(width - 2));
        queue!(
            out,
            MoveTo(x as u16, (y + i) as u16),
            SetForegroundColor(Color::DarkGrey),
            Print(frame)
        )?;
    }
    queue!(
        out,
        MoveTo((x + 2) as u16, y as u16),
        SetForegroundColor(Color::Cyan),
        Print(cut(title, width.saturating_sub(4)))
    )?;
    for (i, line) in lines.iter().take(height.saturating_sub(4)).enumerate() {
        let heading = line.starts_with("# ");
        let txt = if heading { &line[2..] } else { line.as_str() };
        queue!(
            out,
            MoveTo((x + 2) as u16, (y + 2 + i) as u16),
            SetForegroundColor(if heading { Color::Cyan } else { Color::Grey }),
            Print(cut(txt, width.saturating_sub(4)))
        )?;
    }
    queue!(
        out,
        MoveTo((x + 2) as u16, (y + height - 2) as u16),
        SetForegroundColor(Color::DarkGrey),
        Print("Esc to close"),
        ResetColor
    )?;
    out.flush()?;
    Ok(())
}
fn help(out: &mut impl Write, _editor: &Editor) -> Result<()> {
    let lines = vec![
        "# Views and navigation",
        "Tab switch focused plane · z/x/c/y toggle planes",
        "f / Enter single plane or restore all three",
        "↑↓ move focused slice · ←→ move linked plane · [ ] stride",
        "Shift+arrows / Shift+drag / middle-drag pan",
        "Ctrl+wheel / +/- zoom · 0 reset zoom",
        "r rotate 90° · h horizontal flip · v vertical flip",
        "# Segmentation",
        "a create/name segmentation · F2 rename · ; next segment",
        "1–9 select segment · Shift+1–9 toggle visibility",
        "Left-drag paint sphere · right-drag erase · e keyboard erase",
        "Shift+hover update crosshairs without painting",
        "Shift+wheel / , . brush diameter · o/O opacity",
        "t cycle effect: off / set inside / set outside",
        "I set target intensity · = sample nearby intensity",
        "l segmentation list · u/Ctrl+Z/Cmd+Z undo",
        "U/Ctrl+R/Ctrl+Shift+Z/Cmd+Shift+Z redo",
        "# Images and output",
        "n load aligned NIfTI · Tab file picker completion",
        "\\ cycle displayed image · m/M edit min/max · w edit range",
        "i selected image information · p preview edited output",
        "b show/hide existing mask · B toggle binary mask export · s save",
        "q return to viewer · ? help",
    ]
    .iter()
    .map(|v| (*v).to_string())
    .collect::<Vec<_>>();
    show_text_menu(out, "niv edit · help", &lines)
}
fn segment_list(out: &mut impl Write, editor: &Editor) -> Result<()> {
    let mut lines = vec![
        "# Segmentations".into(),
        "↑↓ select · Space toggle · F2 rename · Esc close".into(),
        String::new(),
    ];
    for (i, s) in editor.segments.iter().enumerate() {
        lines.push(format!(
            "{} {:02} {:<20} {}  {}={:.5}",
            if i == editor.active_seg { "❯" } else { " " },
            i + 1,
            s.name,
            if s.visible { "shown" } else { "hidden" },
            s.region.label(),
            s.fill_value
        ));
    }
    if editor.segments.is_empty() {
        lines.push("No segments: press a to create one".into());
    }
    show_text_menu(out, "niv edit · segments", &lines)?;
    let (cols, rows) = terminal::size()?;
    let width = (lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(40)
        + 6)
    .min(cols.saturating_sub(2) as usize)
    .max(24);
    let height = (lines.len() + 5)
        .min(rows.saturating_sub(2) as usize)
        .max(5);
    let x = (cols as usize).saturating_sub(width) / 2;
    let y = (rows as usize).saturating_sub(height) / 2;
    for (i, seg) in editor.segments.iter().enumerate() {
        if i + 3 >= height.saturating_sub(4) {
            break;
        }
        queue!(
            out,
            MoveTo((x + 2) as u16, (y + i + 5) as u16),
            SetForegroundColor(Color::Rgb {
                r: seg.color[0],
                g: seg.color[1],
                b: seg.color[2]
            }),
            Print(cut(&lines[i + 3], width.saturating_sub(4))),
            ResetColor
        )?;
    }
    out.flush()?;
    Ok(())
}
fn image_info(out: &mut impl Write, editor: &Editor) -> Result<()> {
    let vol = editor.selected_volume();
    let lines = vec![
        "# Selected NIfTI".to_owned(),
        format!("Path: {}", editor.selected_path().display()),
        format!("Dimensions: {:?} voxels", vol.dims()),
        format!("Voxel spacing: {:?}", vol.spacing),
        format!("Spatial units: {}", vol.spatial_units),
        format!("Datatype code: {}", vol.datatype),
        format!(
            "Display min/max: {:.7}, {:.7}",
            editor.selected_window().0,
            editor.selected_window().1
        ),
        format!(
            "Focused view: {} · rotation {}° · flips h={} v={}",
            ["X", "Y", "Z"][editor.active_view],
            editor.rotation[editor.active_view] as usize * 90,
            editor.flip_h[editor.active_view],
            editor.flip_v[editor.active_view]
        ),
        format!("Output directory: {}", editor.out_dir.display()),
    ];
    show_text_menu(out, "niv edit · image information", &lines)
}
fn file_picker(out: &mut impl Write, picker: &PickerState) -> Result<()> {
    let count = picker.matches.values.len();
    let (cols, rows) = terminal::size()?;
    let visible = rows.saturating_sub(11) as usize;
    let selected = picker.matches.selected_index();
    let start = selected
        .saturating_sub(visible / 2)
        .min(count.saturating_sub(visible));
    let mut lines = vec![
        "# Search NIfTI image".into(),
        format!("Filter: {}▏", picker.text),
        format!(
            "{} matches in {}",
            count,
            if picker.matches.directory.is_empty() {
                "."
            } else {
                &picker.matches.directory
            }
        ),
        String::new(),
    ];
    for (i, item) in picker
        .matches
        .values
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
    {
        lines.push(format!(
            "{} {}",
            if i == selected { "❯" } else { " " },
            item.path
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "↑↓ select · Enter open · ← parent · Tab complete · Esc cancel · {cols} cols"
    ));
    show_text_menu(out, "niv edit · load image", &lines)
}
enum PickerAction {
    None,
    Load(PathBuf),
    Cancel,
}
fn picker_key(picker: &mut PickerState, key: event::KeyEvent) -> PickerAction {
    match key.code {
        KeyCode::Esc => return PickerAction::Cancel,
        KeyCode::Up => picker.matches.move_by(-1),
        KeyCode::Down => picker.matches.move_by(1),
        KeyCode::Tab => path_complete::advance(&mut picker.text, &mut picker.matches, false),
        KeyCode::BackTab => path_complete::advance(&mut picker.text, &mut picker.matches, true),
        KeyCode::Left => {
            let mut path = path_complete::resolve(&picker.text);
            if !path.pop() {
                picker.text.clear();
            } else {
                picker.text = format!("{}/", path.display());
            }
            picker.refresh();
        }
        KeyCode::Right | KeyCode::Enter => {
            let path = picker
                .matches
                .selected()
                .map(|m| m.path.clone())
                .unwrap_or_else(|| picker.text.clone());
            let resolved = path_complete::resolve(&path);
            if resolved.is_dir() {
                picker.text = if path.ends_with('/') {
                    path
                } else {
                    format!("{path}/")
                };
                picker.refresh();
            } else if path_complete::is_nifti_file(&resolved) {
                return PickerAction::Load(resolved);
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.text.clear();
            picker.refresh();
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
        {
            picker.text.push(c);
            picker.refresh();
        }
        KeyCode::Backspace => {
            picker.text.pop();
            picker.refresh();
        }
        _ => {}
    }
    PickerAction::None
}
/// Start in the same NIfTI file browser when `niv` is invoked without paths.
pub fn pick_first_nifti() -> Result<Option<PathBuf>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("niv requires an interactive terminal when no input file is given");
    }
    let _guard = crate::TerminalGuard::enter(true, false)?;
    let mut out = io::stdout().lock();
    let mut picker = PickerState::open();
    loop {
        file_picker(&mut out, &picker)?;
        match event::read()? {
            Event::Key(key) if key.kind != event::KeyEventKind::Release => {
                match picker_key(&mut picker, key) {
                    PickerAction::Load(path) => return Ok(Some(path)),
                    PickerAction::Cancel => return Ok(None),
                    PickerAction::None => {}
                }
            }
            _ => {}
        }
    }
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
    let editor = Editor::load(args)?;
    let _guard = crate::TerminalGuard::enter(true, false)?;
    let mut stdout = io::stdout().lock();
    run_loop(editor, &mut stdout)
}

fn run_loop(mut editor: Editor, stdout: &mut impl Write) -> Result<()> {
    execute!(stdout, Clear(ClearType::All))?;
    let mut active_image = None;
    let mut next_id = 5_000_000u32;
    let mut current_canvas = canvas(&editor.source, &editor)?;
    let mut redraw = true;
    let mut help_open = false;
    let mut last_pan: Option<(usize, u16, u16, MouseButton)> = None;
    let mut last_frame = Instant::now() - Duration::from_millis(100);
    let frame_interval = Duration::from_millis(40);
    let mut dragging: Option<(MouseButton, [usize; 3])> = None;
    loop {
        if redraw && last_frame.elapsed() >= frame_interval {
            if let Some(picker) = editor.picker.as_ref() {
                file_picker(stdout, picker)?;
            } else if help_open {
                help(stdout, &editor)?;
            } else if editor.info_open {
                image_info(stdout, &editor)?;
            } else if editor.seg_list_open {
                segment_list(stdout, &editor)?;
            } else {
                current_canvas = draw(&editor, stdout, &mut active_image, &mut next_id)?;
            }
            redraw = false;
            last_frame = Instant::now();
        }
        if !event::poll(Duration::from_millis(15))? {
            continue;
        }
        match event::read()? {
            Event::Resize(_, _) => redraw = true,
            Event::Key(key) => {
                if key.kind == event::KeyEventKind::Release {
                    continue;
                }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                if let Some(picker) = editor.picker.as_mut() {
                    match picker_key(picker, key) {
                        PickerAction::None => {}
                        PickerAction::Cancel => {
                            editor.picker = None;
                            execute!(stdout, Clear(ClearType::All))?;
                        }
                        PickerAction::Load(path) => {
                            editor.picker = None;
                            if let Err(err) = editor.add_image(&path) {
                                editor.notice = format!("Cannot load image: {err:#}");
                            }
                            execute!(stdout, Clear(ClearType::All))?;
                        }
                    }
                    redraw = true;
                    continue;
                }
                if help_open || editor.info_open {
                    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | '?' | 'i'))
                        || (key.code == KeyCode::Char('/')
                            && key.modifiers.contains(KeyModifiers::SHIFT))
                    {
                        help_open = false;
                        editor.info_open = false;
                        execute!(stdout, Clear(ClearType::All))?;
                        redraw = true;
                    }
                    continue;
                }
                if editor.seg_list_open {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('l') => {
                            editor.seg_list_open = false;
                            execute!(stdout, Clear(ClearType::All))?;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if !editor.segments.is_empty() {
                                editor.active_seg = (editor.active_seg + editor.segments.len() - 1)
                                    % editor.segments.len();
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if !editor.segments.is_empty() {
                                editor.active_seg = (editor.active_seg + 1) % editor.segments.len();
                            }
                        }
                        KeyCode::Char(' ') => editor.seg_visible(),
                        KeyCode::F(2) => {
                            if let Some(seg) = editor.segments.get(editor.active_seg) {
                                editor.edit_value = Some((EntryKind::Name, seg.name.clone()));
                                editor.seg_list_open = false;
                                execute!(stdout, Clear(ClearType::All))?;
                            }
                        }
                        _ => {}
                    }
                    redraw = true;
                    continue;
                }
                if editor.edit_value.is_some() {
                    match key.code {
                        KeyCode::Esc => editor.edit_value = None,
                        KeyCode::Backspace => {
                            if let Some((_, txt)) = editor.edit_value.as_mut() {
                                txt.pop();
                            }
                        }
                        KeyCode::Char('a' | 'u')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            if let Some((_, txt)) = editor.edit_value.as_mut() {
                                txt.clear();
                            }
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
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
                                        if editor.segments.is_empty() {
                                            Err("Create a segmentation with a first".to_string())
                                        } else {
                                            editor.segments[editor.active_seg].fill_value = v;
                                            editor.rebuild_all();
                                            editor.dirty = true;
                                            Ok(format!("Target intensity = {v}"))
                                        }
                                    }
                                    _ => Err("Intensity must be finite and nonnegative".into()),
                                },
                                EntryKind::Name => {
                                    if !string.is_empty() {
                                        editor.seg_name(&string);
                                    }
                                    Ok(format!(
                                        "Segment: {}",
                                        editor
                                            .segments
                                            .get(editor.active_seg)
                                            .map(|s| s.name.as_str())
                                            .unwrap_or("none")
                                    ))
                                }
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
                                            editor.set_selected_window((lo, hi));
                                            Ok(format!("Display window {lo}..{hi}"))
                                        }
                                        _ => Err("Enter min,max with min < max".into()),
                                    }
                                }
                                EntryKind::Minimum | EntryKind::Maximum => {
                                    match string.parse::<f32>() {
                                        Ok(v) if v.is_finite() => {
                                            let (lo, hi) = editor.selected_window();
                                            let window = if entry == EntryKind::Minimum {
                                                (v, hi)
                                            } else {
                                                (lo, v)
                                            };
                                            if window.0 < window.1 {
                                                editor.set_selected_window(window);
                                                Ok(format!(
                                                    "Display window {:.5}..{:.5}",
                                                    window.0, window.1
                                                ))
                                            } else {
                                                Err("Minimum must be smaller than maximum".into())
                                            }
                                        }
                                        _ => Err("Invalid finite display value".into()),
                                    }
                                }
                            };
                            editor.notice = match answer {
                                Ok(m) | Err(m) => m,
                            };
                        }
                        _ => {}
                    }
                    redraw = true;
                    continue;
                }
                // Keyboard enhancement can report shifted characters as either
                // uppercase letters or lowercase letters with SHIFT set.
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                let cmd = key.modifiers.contains(KeyModifiers::SUPER);
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if (cmd || ctrl) && matches!(key.code, KeyCode::Char('z' | 'Z')) {
                    if shift || key.code == KeyCode::Char('Z') {
                        editor.redo();
                    } else {
                        editor.undo();
                    }
                    redraw = true;
                    continue;
                }
                if ctrl && matches!(key.code, KeyCode::Char('r' | 'R')) {
                    editor.redo();
                    redraw = true;
                    continue;
                }
                if let KeyCode::Char(c) = key.code {
                    // Support both Kitty-shift-modified digits and legacy
                    // terminal !@#... sequences for toggling visibility.
                    let digit = if c.is_ascii_digit() {
                        c.to_digit(10)
                    } else {
                        "!@#$%^&*("
                            .chars()
                            .position(|mark| mark == c)
                            .map(|i| i as u32 + 1)
                    };
                    if let Some(n) = digit {
                        if n == 0 {
                            let a = editor.active_view;
                            editor.zoom[a] = 1.;
                            let (h, v) = axes(a);
                            let dims = editor.dims();
                            editor.centers[a] =
                                [(dims[h] - 1) as f64 / 2., (dims[v] - 1) as f64 / 2.];
                        } else if (n as usize) <= editor.segments.len() {
                            if shift || !c.is_ascii_digit() {
                                editor.segments[n as usize - 1].visible =
                                    !editor.segments[n as usize - 1].visible;
                            } else {
                                editor.finish_stroke();
                                editor.active_seg = n as usize - 1;
                            }
                        }
                        redraw = true;
                        continue;
                    }
                }
                match key.code {
                    KeyCode::Char('?') | KeyCode::Char('/')
                        if key.code == KeyCode::Char('?') || shift =>
                    {
                        editor.finish_stroke();
                        if let Some(id) = active_image.take() {
                            kitty::delete(stdout, id)?;
                        }
                        help_open = true;
                        execute!(stdout, Clear(ClearType::All))?;
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
                    KeyCode::Tab | KeyCode::BackTab => {
                        let advance = if key.code == KeyCode::Tab { 1 } else { 2 };
                        for _ in 0..3 {
                            editor.active_view = (editor.active_view + advance) % 3;
                            if editor.visible_views[editor.active_view] {
                                break;
                            }
                        }
                        editor.hover = None;
                    }
                    KeyCode::Char('x' | 'X' | 'c' | 'C' | 'y' | 'Y' | 'z' | 'Z') => {
                        let axis = match key.code {
                            KeyCode::Char('x' | 'X') => 0,
                            KeyCode::Char('c' | 'C' | 'y' | 'Y') => 1,
                            _ => 2,
                        };
                        editor.toggle_view(axis);
                    }
                    KeyCode::Char('f') | KeyCode::Enter => {
                        if editor.visible_views.iter().filter(|&&v| v).count() == 1 {
                            editor.visible_views = [true; 3];
                        } else {
                            editor.visible_views = [false; 3];
                            editor.visible_views[editor.active_view] = true;
                        }
                        editor.hover = None;
                    }
                    KeyCode::Char('r') => {
                        editor.rotation[editor.active_view] =
                            (editor.rotation[editor.active_view] + 1) % 4
                    }
                    KeyCode::Char('h') => {
                        editor.flip_h[editor.active_view] = !editor.flip_h[editor.active_view]
                    }
                    KeyCode::Char('v') => {
                        editor.flip_v[editor.active_view] = !editor.flip_v[editor.active_view]
                    }
                    KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Char('j' | 'k') => {
                        let axis = editor.active_view;
                        let mult = if shift {
                            4
                        } else if ctrl {
                            3
                        } else if key.modifiers.contains(KeyModifiers::ALT) {
                            2
                        } else {
                            1
                        };
                        let step = editor.stride * mult;
                        if shift {
                            let (dx, dy) = match key.code {
                                KeyCode::Left => (-1., 0.),
                                KeyCode::Right => (1., 0.),
                                KeyCode::Up | KeyCode::Char('k') => (0., 1.),
                                _ => (0., -1.),
                            };
                            let nat = native_from_display(&editor, axis, [dx, dy]);
                            editor.pan(
                                axis,
                                -nat[0] * editor.stride as f64,
                                -nat[1] * editor.stride as f64,
                            );
                        } else if key.code == KeyCode::Left || key.code == KeyCode::Right {
                            let (h, _) = axes(axis);
                            editor.move_slice(
                                h,
                                if key.code == KeyCode::Right {
                                    step as isize
                                } else {
                                    -(step as isize)
                                },
                            );
                        } else {
                            let forward = matches!(key.code, KeyCode::Down | KeyCode::Char('j'));
                            editor.move_slice(
                                axis,
                                if forward {
                                    step as isize
                                } else {
                                    -(step as isize)
                                },
                            );
                        }
                        editor.hover = None;
                    }
                    KeyCode::Char('[') => editor.stride = editor.stride.saturating_sub(1).max(1),
                    KeyCode::Char(']') => editor.stride = (editor.stride + 1).min(256),
                    KeyCode::Char(',' | '<') => editor.radius(-12.5),
                    KeyCode::Char('.' | '>') => editor.radius(12.5),
                    KeyCode::Char('+') => editor.change_zoom(0.10),
                    KeyCode::Char('-') => editor.change_zoom(-0.10),
                    KeyCode::Char('=') => editor.sample_fill(),
                    KeyCode::Char('p') => editor.preview_output = !editor.preview_output,
                    KeyCode::Char('e') => {
                        editor.tool = if editor.tool == Tool::Paint {
                            Tool::Erase
                        } else {
                            Tool::Paint
                        };
                        editor.notice = format!(
                            "Keyboard brush: {}",
                            if editor.tool == Tool::Paint {
                                "paint"
                            } else {
                                "erase"
                            }
                        );
                    }
                    KeyCode::Char(' ') => {
                        editor.begin_stroke();
                        editor.paint_sphere(editor.coordinates, editor.tool == Tool::Erase);
                        editor.finish_stroke();
                    }
                    KeyCode::Char('a') => {
                        editor.new_segment();
                        editor.edit_value = Some((EntryKind::Name, String::new()));
                    }
                    KeyCode::F(2) => {
                        if let Some(seg) = editor.segments.get(editor.active_seg) {
                            editor.edit_value = Some((EntryKind::Name, seg.name.clone()));
                        }
                    }
                    KeyCode::Char('n') => {
                        if let Some(id) = active_image.take() {
                            kitty::delete(stdout, id)?;
                        }
                        editor.picker = Some(PickerState::open());
                        execute!(stdout, Clear(ClearType::All))?;
                    }
                    KeyCode::Char('\\') => editor.cycle_image(),
                    KeyCode::Char(';') => {
                        if !editor.segments.is_empty() {
                            editor.finish_stroke();
                            editor.active_seg = (editor.active_seg + 1) % editor.segments.len();
                        }
                    }
                    KeyCode::Char('l') => {
                        if let Some(id) = active_image.take() {
                            kitty::delete(stdout, id)?;
                        }
                        editor.seg_list_open = true;
                        execute!(stdout, Clear(ClearType::All))?;
                    }
                    KeyCode::Char('t') => editor.cycle_apply_region(),
                    KeyCode::Char('I') => {
                        if let Some(seg) = editor.segments.get(editor.active_seg) {
                            editor.edit_value =
                                Some((EntryKind::Fill, format!("{}", seg.fill_value)));
                        }
                    }
                    KeyCode::Char('i') if shift => {
                        if let Some(seg) = editor.segments.get(editor.active_seg) {
                            editor.edit_value =
                                Some((EntryKind::Fill, format!("{}", seg.fill_value)));
                        }
                    }
                    KeyCode::Char('i') => {
                        if let Some(id) = active_image.take() {
                            kitty::delete(stdout, id)?;
                        }
                        editor.info_open = true;
                        execute!(stdout, Clear(ClearType::All))?;
                    }
                    KeyCode::Char('o') if shift => {
                        editor.overlay_opacity = (editor.overlay_opacity + 0.05).min(1.)
                    }
                    KeyCode::Char('o') => {
                        editor.overlay_opacity = (editor.overlay_opacity - 0.05).max(0.)
                    }
                    KeyCode::Char('O') => {
                        editor.overlay_opacity = (editor.overlay_opacity + 0.05).min(1.)
                    }
                    KeyCode::Char('m') if shift => {
                        editor.edit_value = Some((
                            EntryKind::Maximum,
                            format!("{}", editor.selected_window().1),
                        ));
                    }
                    KeyCode::Char('m') => {
                        editor.edit_value = Some((
                            EntryKind::Minimum,
                            format!("{}", editor.selected_window().0),
                        ));
                    }
                    KeyCode::Char('M') => {
                        editor.edit_value = Some((
                            EntryKind::Maximum,
                            format!("{}", editor.selected_window().1),
                        ));
                    }
                    KeyCode::Char('w') => {
                        let (lo, hi) = editor.selected_window();
                        editor.edit_value = Some((EntryKind::Window, format!("{lo},{hi}")));
                    }
                    KeyCode::Char('b') => editor.show_mask = !editor.show_mask,
                    KeyCode::Char('B') => {
                        editor.export_mask = !editor.export_mask;
                        editor.notice = format!("Binary brain-mask export: {}", editor.export_mask);
                    }
                    KeyCode::Char('u') if shift => editor.redo(),
                    KeyCode::Char('u') => editor.undo(),
                    KeyCode::Char('U') => editor.redo(),
                    KeyCode::Char('s') => {
                        if let Err(e) = editor.save() {
                            editor.notice = format!("Save error: {e:#}");
                        }
                    }
                    _ => {}
                }
                redraw = true;
            }
            Event::Mouse(mouse)
                if !help_open
                    && !editor.seg_list_open
                    && !editor.info_open
                    && editor.picker.is_none() =>
            {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && mouse.row == HEADER_ROWS + current_canvas.geom.rows + 2
                {
                    if let Some(i) =
                        footer_image_at(&editor, &current_canvas, mouse.column as usize)
                    {
                        editor.selected_image = i;
                        redraw = true;
                    }
                    continue;
                }
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
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                            editor.radius(-sign * 25.);
                        } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                            editor.change_zoom(-sign * 0.10);
                        } else {
                            editor.move_slice(
                                editor.active_view,
                                if up {
                                    -(editor.stride as isize)
                                } else {
                                    editor.stride as isize
                                },
                            );
                        }
                        redraw = true;
                    }
                    MouseEventKind::Moved => {
                        if let Some((axis, p)) = hit {
                            editor.hover = Some(p);
                            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                                // No brush-distance threshold: update for every
                                // newly sampled voxel under the cursor.
                                editor.move_crosshair_in_plane(axis, p);
                            }
                            redraw = true;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Middle) => {
                        if let Some((axis, p)) = hit {
                            editor.active_view = axis;
                            editor.hover = Some(p);
                            last_pan = Some((axis, mouse.column, mouse.row, MouseButton::Middle));
                            redraw = true;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) =>
                    {
                        if let Some((axis, p)) = hit {
                            editor.active_view = axis;
                            editor.hover = Some(p);
                            last_pan = Some((axis, mouse.column, mouse.row, MouseButton::Left));
                            redraw = true;
                        }
                    }
                    MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) => {
                        if let Some((axis, p)) = hit {
                            editor.active_view = axis;
                            editor.hover = Some(p);
                            editor.coordinates = p;
                            if editor.segments.is_empty() {
                                editor.notice = "Press a to create a segmentation first".into();
                            } else {
                                editor.begin_stroke();
                                editor.paint_sphere(p, button == MouseButton::Right);
                                dragging = Some((button, p));
                            }
                            redraw = true;
                        }
                    }
                    MouseEventKind::Drag(button) => {
                        if last_pan.is_none()
                            && button == MouseButton::Left
                            && mouse.modifiers.contains(KeyModifiers::SHIFT)
                        {
                            if let Some((axis, p)) = hit {
                                editor.active_view = axis;
                                editor.hover = Some(p);
                                last_pan = Some((axis, mouse.column, mouse.row, button));
                                dragging = None;
                            }
                        }
                        if let Some((axis, col, row, pan_button)) = last_pan {
                            if button == pan_button {
                                if let Some(pane) =
                                    current_canvas.panes.iter().find(|pane| pane.axis == axis)
                                {
                                    let dx = (mouse.column as f64 - col as f64)
                                        * current_canvas.width as f64
                                        / current_canvas.cols as f64;
                                    let dy = (mouse.row as f64 - row as f64)
                                        * current_canvas.height as f64
                                        / current_canvas.geom.rows as f64;
                                    pan_screen(&mut editor, pane, dx, dy);
                                }
                                last_pan = Some((axis, mouse.column, mouse.row, pan_button));
                                redraw = true;
                            }
                        } else if let (Some((axis, p)), Some((started, last))) = (hit, dragging) {
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
                        last_pan = None;
                        redraw = true;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(id) = active_image {
        kitty::delete(stdout, id)?;
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
            mask_path: Some(PathBuf::from("mask.nii.gz")),
            export_mask: true,
            raw: None,
            extra_images: Vec::new(),
            selected_image: 0,
            picker: None,
            info_open: false,
            preview_output: false,
            segments: Vec::new(),
            active_seg: 0,
            active_view: 0,
            visible_views: [true; 3],
            zoom: [1.; 3],
            centers: [
                [dims[1] as f64 / 2., dims[2] as f64 / 2.],
                [dims[0] as f64 / 2., dims[2] as f64 / 2.],
                [dims[0] as f64 / 2., dims[1] as f64 / 2.],
            ],
            rotation: [0; 3],
            flip_h: [false; 3],
            flip_v: [false; 3],
            stride: 1,
            tool: Tool::Paint,
            show_mask: true,
            overlay_opacity: 0.22,
            seg_list_open: false,
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
        e.new_segment();
        e.segments[0].region = ApplyRegion::Inside;
        e
    }
    #[test]
    fn image_alone_is_valid_and_segments_are_not_assumed() {
        let args = parse_args(&["image.nii.gz".into()]).unwrap().unwrap();
        assert!(args.mask.is_none() && !args.binarize_mask);
        assert_eq!(args.masked, PathBuf::from("image.nii.gz"));
        let args = parse_args(&[
            "--masked".into(),
            "image.nii.gz".into(),
            "--binarize-mask".into(),
        ])
        .unwrap()
        .unwrap();
        assert!(args.binarize_mask);
    }
    #[test]
    fn optional_mask_export_does_not_write_an_unrequested_file() {
        let mut e = synthetic([5, 5, 5], [50.; 3]);
        e.export_mask = false;
        e.mask_path = None;
        assert_eq!(e.output_paths().len(), 2); // edited volume + initial segment
        e.export_mask = true;
        assert_eq!(e.output_paths().len(), 3);
        assert!(e.output_paths()[1].to_string_lossy().contains("brain_mask"));
    }
    #[test]
    fn plane_visibility_and_zoom_are_independent() {
        let mut e = synthetic([11, 12, 13], [50.; 3]);
        e.active_view = 0;
        e.change_zoom(0.5);
        assert_eq!(e.zoom, [1.5, 1., 1.]);
        e.toggle_view(0);
        assert_eq!(e.visible_views, [false, true, true]);
        e.toggle_view(0);
        assert_eq!(e.visible_views, [true, true, true]);
        e.pan(0, 4., 2.);
        assert_ne!(e.centers[0], e.centers[1]);
    }
    #[test]
    fn saved_segments_remember_visibility_and_green_trim_palette() {
        let meta = SegmentMetadata {
            version: 1,
            role: "trim".into(),
            fill_value: 0.,
            color: palette(Role::Trim, 0),
            visible: false,
            region: Some(ApplyRegion::Inside),
        };
        let restored: SegmentMetadata = toml::from_str(&toml::to_string(&meta).unwrap()).unwrap();
        assert!(!restored.visible);
        assert!(restored.color[1] > restored.color[0]);
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
        e.new_segment();
        e.segments[e.active_seg].fill_value = 37.;
        e.segments[e.active_seg].region = ApplyRegion::Inside;
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
            visible: true,
            region: Some(ApplyRegion::Outside),
        };
        let serialized = toml::to_string_pretty(&metadata).unwrap();
        let restored: SegmentMetadata = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.fill_value, 37.125);
        assert_eq!(restored.color, [24, 210, 190]);
        assert_eq!(restored.role, "fill");
    }
    #[test]
    fn generic_segment_inside_outside_and_overlay_keep_source_intact() {
        let mut e = synthetic([9, 9, 9], [50.; 3]);
        let center = e.coordinates;
        e.paint_sphere(center, false);
        e.finish_stroke();
        assert_eq!(e.source.data[center], 10.);
        assert_eq!(e.work[center], 0.);
        assert_eq!(e.selected_volume().data[center], 10.);
        let c = colorized(&e, center);
        assert!(c[1] > c[0]); // green translucent overlay over original gray
        e.segments[0].fill_value = 17.;
        e.segments[0].region = ApplyRegion::Outside;
        e.rebuild_all();
        assert_eq!(e.work[center], 10.);
        assert_eq!(e.work[[0, 0, 0]], 17.);
        e.segments[0].region = ApplyRegion::Off;
        e.rebuild_all();
        assert_eq!(e.work[[0, 0, 0]], 10.);
    }
    #[test]
    fn orientation_view_roundtrip_works_with_rotations_and_flips() {
        let mut e = synthetic([13, 15, 17], [50.; 3]);
        let pane = Pane {
            axis: 1,
            x: 20,
            y: 10,
            w: 400,
            h: 360,
        };
        for rot in 0..4 {
            e.rotation[1] = rot;
            for flip in [false, true] {
                e.flip_h[1] = flip;
                e.flip_v[1] = !flip;
                let p = [8, 7, 11];
                let (x, y) = pixel_for(&e, &pane, p);
                let q = voxel_at(&e, &pane, x as usize, y as usize).unwrap();
                assert!((q[0] as isize - p[0] as isize).abs() <= 1);
                assert!((q[2] as isize - p[2] as isize).abs() <= 1);
            }
        }
    }
    #[test]
    fn selected_image_window_does_not_modify_edit_source_window() {
        let mut e = synthetic([7; 3], [50.; 3]);
        let other = Volume {
            data: e.source.data.clone(),
            spacing: [50.; 3],
            spatial_units: 3,
            sform: None,
            datatype: 16,
        };
        e.extra_images.push(ImageLayer {
            path: "extra.nii.gz".into(),
            window: (2., 11.),
            volume: other,
        });
        e.selected_image = 1;
        e.set_selected_window((4., 8.));
        assert_eq!(e.windows, (0., 100.));
        assert_eq!(e.selected_window(), (4., 8.));
        e.cycle_image();
        assert_eq!(e.selected_image, 0);
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
