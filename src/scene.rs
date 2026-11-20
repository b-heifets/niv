//! Human-editable, versioned TOML scenes. A scene is configuration, not executable
//! shell commands; paths are resolved relative to the scene file when possible.
use crate::{
    app::{App, MontageState},
    config::{Config, OverlaySpec},
    layers::{Layers, Palette},
    volume::Axis,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneFile {
    version: u32,
    view: SceneView,
    render: SceneRender,
    #[serde(skip_serializing_if = "Option::is_none")]
    atlas: Option<SceneAtlas>,
    // Ordered top-to-bottom, matching the `t` layer list. Atlas stays above all.
    layers: Vec<SceneImage>,
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneView {
    axis: String,
    slice: usize,
    rotation: u16,
    stride: usize,
    flip_h: bool,
    flip_v: bool,
    selected: usize,
    show_overlays: bool,
    fullscreen: bool,
    inspect_mode: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe: Option<[usize; 2]>,
    /// Optional for backward compatibility with v0.14 scene files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    montage: Option<SceneMontage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneMontage {
    enabled: bool,
    start: usize,
    stop: usize,
    interval: usize,
    columns: usize,
    gap: usize,
    #[serde(default = "default_montage_rows")]
    rows: usize,
    #[serde(default)]
    show_numbers: bool,
    #[serde(default)]
    show_clusters: bool,
}
fn default_montage_rows() -> usize {
    3
}

impl From<MontageState> for SceneMontage {
    fn from(m: MontageState) -> Self {
        Self {
            enabled: m.enabled,
            start: m.start,
            stop: m.stop,
            interval: m.interval,
            columns: m.columns,
            gap: m.gap,
            rows: m.rows,
            show_numbers: m.show_numbers,
            show_clusters: m.show_clusters,
        }
    }
}
impl From<&SceneMontage> for MontageState {
    fn from(m: &SceneMontage) -> Self {
        Self {
            enabled: m.enabled,
            start: m.start,
            stop: m.stop,
            interval: m.interval,
            columns: m.columns,
            gap: m.gap,
            rows: m.rows,
            show_numbers: m.show_numbers,
            show_clusters: m.show_clusters,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneRender {
    background: String,
    pixel_budget: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    montage_pixel_budget: Option<usize>,
    fps: u32,
    hover_delay_ms: u64,
    mouse_pixels: bool,
    enhanced_keys: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneAtlas {
    path: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lut: Option<String>,
    color: String,
    opacity: f32,
    width: usize,
    aa: bool,
    #[serde(default)]
    blur: f32,
    visible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneImage {
    path: String,
    role: String,
    window: [f32; 2],
    opacity: f32,
    palette: String,
    visible: bool,
}

fn valid_window([low, high]: [f32; 2]) -> bool {
    low.is_finite() && high.is_finite() && low < high
}
fn valid_opacity(opacity: f32) -> bool {
    opacity.is_finite() && (0.0..=1.0).contains(&opacity)
}
fn encode_color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}
fn parse_color(s: &str) -> Result<[u8; 3]> {
    let hex = s.trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        bail!("Invalid RGB color {s:?}; expected #RRGGBB");
    }
    Ok([
        u8::from_str_radix(&hex[0..2], 16)?,
        u8::from_str_radix(&hex[2..4], 16)?,
        u8::from_str_radix(&hex[4..6], 16)?,
    ])
}
fn palette_name(p: Palette) -> String {
    match p {
        Palette::Gray => "gray".into(),
        Palette::Warm => "warm".into(),
        Palette::Cool => "cool".into(),
        Palette::Random => "categorical".into(),
        Palette::Solid(rgb) => format!("tint:{}", encode_color(rgb)),
    }
}
fn parse_palette(text: &str) -> Result<Palette> {
    Ok(match text {
        "gray" => Palette::Gray,
        "warm" => Palette::Warm,
        "cool" => Palette::Cool,
        "categorical" => Palette::Random,
        _ if text.starts_with("tint:") => Palette::Solid(parse_color(&text[5..])?),
        _ => bail!("Unknown palette {text:?}; expected gray/warm/cool/categorical/tint:#RRGGBB"),
    })
}
fn absolute(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}
fn reference_path(file: &Path, scene_dir: &Path) -> String {
    let source = absolute(file);
    let dir = absolute(scene_dir);
    source
        .strip_prefix(&dir)
        .unwrap_or(&source)
        .to_string_lossy()
        .into_owned()
}
fn resolve_path(scene_path: &Path, text: &str) -> PathBuf {
    let p = Path::new(text);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        scene_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(p)
    }
}

impl SceneFile {
    pub fn capture(path: &Path, config: &Config, app: &App, layers: &Layers) -> Self {
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let entries = app
            .layer_order
            .iter()
            .rev()
            .map(|&id| {
                let (source, window, opacity, palette, visible, role) = if id == 0 {
                    (
                        &config.path,
                        app.window,
                        app.base_opacity,
                        app.base_palette,
                        app.base_visible,
                        "base",
                    )
                } else {
                    let layer = &layers.overlays[id - 1];
                    (
                        &layer.path,
                        layer.window,
                        layer.opacity,
                        layer.palette,
                        layer.visible,
                        "overlay",
                    )
                };
                SceneImage {
                    path: reference_path(source, dir),
                    role: role.into(),
                    window: [window.0, window.1],
                    opacity,
                    palette: palette_name(palette),
                    visible,
                }
            })
            .collect();
        let atlas = layers.atlas.as_ref().and_then(|a| {
            config.atlas_path.as_ref().map(|source| SceneAtlas {
                path: reference_path(source, dir),
                kind: if a.wireframe { "wireframe" } else { "solid" }.into(),
                labels: config
                    .atlas_labels_path
                    .as_ref()
                    .map(|p| reference_path(p, dir)),
                lut: config.lut_path.as_ref().map(|p| reference_path(p, dir)),
                color: encode_color(a.fallback),
                opacity: a.opacity,
                width: a.width,
                aa: app.atlas_aa,
                blur: app.atlas_blur,
                visible: app.show_atlas,
            })
        });
        Self {
            version: FORMAT_VERSION,
            view: SceneView {
                axis: app.axis.label().into(),
                slice: app.index,
                rotation: app.rotation,
                stride: app.stride,
                flip_h: app.flip_h,
                flip_v: app.flip_v,
                selected: app.layer_order.len() - 1 - app.selected,
                show_overlays: app.show_overlays,
                fullscreen: app.fullscreen,
                inspect_mode: app.inspect_mode,
                probe: app.probe.map(|(x, y)| [x, y]),
                montage: if app.montage_preset_pending {
                    None
                } else {
                    Some(app.montage.into())
                },
            },
            render: SceneRender {
                background: if app.transparent_background {
                    "transparent"
                } else {
                    "black"
                }
                .into(),
                pixel_budget: app.pixel_budget,
                montage_pixel_budget: Some(app.montage_pixel_budget),
                fps: config.max_fps,
                hover_delay_ms: config.hover_delay_ms,
                mouse_pixels: config.mouse_pixels,
                enhanced_keys: config.enhanced_keys,
            },
            atlas,
            layers: entries,
            source: Some(absolute(path)),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.version != FORMAT_VERSION {
            bail!(
                "Unsupported scene version {} (expected {FORMAT_VERSION})",
                self.version
            );
        }
        if self.layers.is_empty() {
            bail!("Scene must contain at least one [[layers]] entry");
        }
        if self.layers.iter().filter(|l| l.role == "base").count() != 1 {
            bail!("Scene must contain exactly one layer with role = \"base\"");
        }
        if self.view.selected >= self.layers.len() {
            bail!("Selected layer is outside stack");
        }
        if let Some(m) = &self.view.montage {
            if m.start > m.stop {
                bail!("Montage start must not exceed stop");
            }
            if m.interval > 256 {
                bail!("Montage interval must be 0..256 (0 = auto)");
            }
            if !(1..=12).contains(&m.rows) {
                bail!("Montage rows must be 1..12");
            }
            if !(1..=12).contains(&m.columns) {
                bail!("Montage columns must be 1..12");
            }
            if m.gap > 32 {
                bail!("Montage gap must be 0..32");
            }
        }
        Axis::parse(&self.view.axis)?;
        if !matches!(self.view.rotation, 0 | 90 | 180 | 270) {
            bail!("Scene rotation must be 0,90,180,270");
        }
        if !(1..=256).contains(&self.view.stride) {
            bail!("Scene stride must be 1..256");
        }
        if !(1..=120).contains(&self.render.fps) {
            bail!("Scene FPS must be 1..120");
        }
        if self
            .render
            .montage_pixel_budget
            .is_some_and(|n| n > 10_000_000)
        {
            bail!("Scene montage pixel budget must be <= 10000000");
        }
        if self.render.pixel_budget > 10_000_000 {
            bail!("Scene pixel budget must be <= 10000000");
        }
        if self.render.hover_delay_ms > 2000 {
            bail!("Scene hover delay must be <= 2000 ms");
        }
        if self.render.background != "black" && self.render.background != "transparent" {
            bail!("Scene background must be black or transparent");
        }
        for (i, l) in self.layers.iter().enumerate() {
            if l.path.is_empty() {
                bail!("Layer {} has an empty path", i + 1);
            }
            if l.role != "base" && l.role != "overlay" {
                bail!("Layer {} has unknown role {:?}", i + 1, l.role);
            }
            if !valid_window(l.window) {
                bail!("Layer {} has an invalid window", i + 1);
            }
            if !valid_opacity(l.opacity) {
                bail!("Layer {} opacity must be 0..1", i + 1);
            }
            parse_palette(&l.palette)?;
        }
        if let Some(a) = &self.atlas {
            if a.path.is_empty() {
                bail!("Atlas path is empty");
            }
            if !matches!(a.kind.as_str(), "solid" | "wireframe") {
                bail!("Atlas kind must be solid or wireframe");
            }
            if a.kind == "solid" && a.labels.is_some() {
                bail!("Atlas labels require a wireframe atlas");
            }
            if a.width < 1 || a.width > 3 {
                bail!("Atlas width must be 1..3");
            }
            if !valid_opacity(a.opacity) {
                bail!("Atlas opacity must be 0..1");
            }
            if !a.blur.is_finite() || !(0.0..=4.0).contains(&a.blur) {
                bail!("Atlas blur must be 0..4 px");
            }
            parse_color(&a.color)?;
        }
        Ok(())
    }

    pub fn apply_to_loaded(&self, app: &mut App, layers: &mut Layers) -> Result<()> {
        self.validate()?;
        let mut ids = Vec::with_capacity(self.layers.len());
        let mut next_overlay = 1usize;
        for img in self.layers.iter().rev() {
            if img.role == "base" {
                ids.push(0);
                app.window = (img.window[0], img.window[1]);
                app.base_opacity = img.opacity;
                app.base_visible = img.visible;
                app.base_palette = parse_palette(&img.palette)?;
            } else {
                let overlay = layers
                    .overlays
                    .get_mut(next_overlay - 1)
                    .context("Scene overlay count does not match loaded images")?;
                ids.push(next_overlay);
                next_overlay += 1;
                overlay.window = (img.window[0], img.window[1]);
                overlay.opacity = img.opacity;
                overlay.palette = parse_palette(&img.palette)?;
                overlay.visible = img.visible;
            }
        }
        if next_overlay != layers.overlays.len() + 1 {
            bail!("Scene overlay count differs from loaded images");
        }
        app.layer_order = ids;
        app.selected = self.layers.len() - 1 - self.view.selected;
        app.flip_h = self.view.flip_h;
        app.flip_v = self.view.flip_v;
        app.show_overlays = self.view.show_overlays;
        app.fullscreen = self.view.fullscreen;
        app.inspect_mode = self.view.inspect_mode;
        app.probe = self.view.probe.map(|[h, v]| (h, v));
        if let Some(m) = &self.view.montage {
            app.montage = m.into();
            app.montage_preset_pending = false;
        }
        if let Some(n) = self.render.montage_pixel_budget {
            app.montage_pixel_budget = n;
        }
        app.scene_path = self.source.clone();
        app.show_atlas = self.atlas.as_ref().is_some_and(|a| a.visible);
        if let (Some(saved), Some(atlas)) = (&self.atlas, &mut layers.atlas) {
            atlas.opacity = saved.opacity;
            app.atlas_aa = saved.aa;
            app.atlas_blur = saved.blur;
        }
        Ok(())
    }
}

pub fn save(path: &Path, config: &Config, app: &App, layers: &Layers) -> Result<()> {
    // Never permit an accidental `s` save to overwrite an input NIfTI image.
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Scene filename must be UTF-8 and end with .niv.toml")?;
    if !filename.ends_with(".niv.toml") {
        bail!("Scene filenames must end with .niv.toml (received {filename:?})");
    }
    let scene = SceneFile::capture(path, config, app, layers);
    scene.validate()?;
    let content = toml::to_string_pretty(&scene).context("Serializing scene")?;
    // Write beside the destination, then rename to avoid leaving a truncated
    // existing scene if serialization or disk writing fails midway.
    let staging = path.with_file_name(format!(".{filename}.tmp-{}", std::process::id()));
    fs::write(
        &staging,
        format!("# niv scene (v1). Paths are relative to this file where possible.\n{content}"),
    )
    .with_context(|| format!("Writing temporary scene {}", staging.display()))?;
    if let Err(err) = fs::rename(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(err).with_context(|| format!("Saving scene {}", path.display()));
    }
    Ok(())
}

pub fn config_from_args(argv: &[String]) -> Result<Config> {
    let mut i = 0;
    let mut filename = None;
    let mut fps = None;
    let mut pixel_budget = None;
    let mut profile = false;
    let mut bench = None;
    fn take_value(
        args: &[String],
        index: &mut usize,
        key: &str,
        inline: Option<&str>,
    ) -> Result<String> {
        if let Some(v) = inline {
            return Ok(v.to_owned());
        }
        *index += 1;
        args.get(*index)
            .cloned()
            .with_context(|| format!("{key} requires a value"))
    }
    while i < argv.len() {
        let arg = &argv[i];
        let (key, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        match key {
            "--scene"=>{
                if filename.is_some(){bail!("Supply exactly one --scene path");}
                filename=Some(PathBuf::from(take_value(argv,&mut i,key,inline)?));
            }
            "--fps"=>fps=Some(take_value(argv,&mut i,key,inline)?.parse::<u32>().context("--fps must be an integer")?),
            "--pixel-budget"=>pixel_budget=Some(take_value(argv,&mut i,key,inline)?.parse::<usize>().context("--pixel-budget must be an integer")?),
            "--profile" if inline.is_none()=>profile=true,
            "--bench"=>bench=Some(take_value(argv,&mut i,key,inline)?.parse::<usize>().context("--bench must be an integer")?),
            _=>bail!("With --scene, only --fps, --pixel-budget, --profile and --bench may override saved settings (got {arg:?})"),
        }
        i += 1;
    }
    let file = filename.context("--scene requires a .niv.toml path")?;
    let text =
        fs::read_to_string(&file).with_context(|| format!("Reading scene {}", file.display()))?;
    let mut scene: SceneFile =
        toml::from_str(&text).with_context(|| format!("Parsing scene {}", file.display()))?;
    scene.validate()?;
    scene.source = Some(absolute(&file));
    let full = scene.source.as_ref().expect("scene has a source");
    let mut base = None;
    let mut overlays = Vec::new();
    for img in scene.layers.iter().rev() {
        let path = resolve_path(full, &img.path);
        if img.role == "base" {
            base = Some(path);
        } else {
            overlays.push(OverlaySpec {
                path,
                window: Some((img.window[0], img.window[1])),
                color: [255, 184, 77],
                grayscale: true,
                opacity: img.opacity,
            });
        }
    }
    let atlas_path = scene.atlas.as_ref().map(|a| resolve_path(full, &a.path));
    let lut_path = scene
        .atlas
        .as_ref()
        .and_then(|a| a.lut.as_deref())
        .map(|p| resolve_path(full, p));
    let labels = scene
        .atlas
        .as_ref()
        .and_then(|a| a.labels.as_deref())
        .map(|p| resolve_path(full, p));
    let fps = fps.unwrap_or(scene.render.fps);
    let pixel_budget = pixel_budget.unwrap_or(scene.render.pixel_budget);
    if !(1..=120).contains(&fps) {
        bail!("--fps must be 1..120");
    }
    if pixel_budget > 10_000_000 {
        bail!("--pixel-budget must be 0..10000000");
    }
    if bench.is_some_and(|n| n == 0 || n > 10_000) {
        bail!("--bench must be 1..10000");
    }
    Ok(Config {
        path: base.context("Scene is missing its reference image")?,
        atlas_only: false,
        axis: Axis::parse(&scene.view.axis)?,
        initial_slice: Some(scene.view.slice),
        step: scene.view.stride,
        rotation: scene.view.rotation,
        window: None,
        base_opacity: 1.0,
        transparent_background: scene.render.background == "transparent",
        hover_delay_ms: scene.render.hover_delay_ms,
        atlas_opacity: scene.atlas.as_ref().map_or(1.0, |a| a.opacity),
        overlays,
        atlas_path,
        atlas_wireframe: scene.atlas.as_ref().is_some_and(|a| a.kind == "wireframe"),
        atlas_color: scene
            .atlas
            .as_ref()
            .map(|a| parse_color(&a.color))
            .transpose()?
            .unwrap_or([255; 3]),
        atlas_width: scene.atlas.as_ref().map_or(1, |a| a.width),
        atlas_aa: scene.atlas.as_ref().is_some_and(|a| a.aa),
        atlas_blur: scene.atlas.as_ref().map_or(0., |a| a.blur),
        atlas_labels_path: labels,
        lut_path,
        mouse_pixels: scene.render.mouse_pixels,
        max_fps: fps,
        enhanced_keys: scene.render.enhanced_keys,
        benchmark_frames: bench,
        pixel_budget,
        profile,
        scene: Some(scene),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_roundtrip() {
        for p in [
            Palette::Gray,
            Palette::Warm,
            Palette::Cool,
            Palette::Random,
            Palette::Solid([14, 172, 250]),
        ] {
            assert_eq!(parse_palette(&palette_name(p)).unwrap(), p);
        }
    }
    #[test]
    fn scene_roundtrip_preserves_reordering_and_relative_paths() {
        let input = r##"
version = 1
[view]
axis = "x"
slice = 41
rotation = 90
stride = 2
flip_h = true
flip_v = false
selected = 0
show_overlays = true
fullscreen = false
inspect_mode = false
[render]
background = "black"
pixel_budget = 1000000
fps = 30
hover_delay_ms = 20
mouse_pixels = false
enhanced_keys = true
[atlas]
path = "atlas/labels.nii.gz"
kind = "solid"
opacity = 0.5
color = "#ff7700"
width = 2
aa = true
visible = true
lut = "atlas/colors.csv"
[[layers]]
path = "overlays/a.nii.gz"
role = "overlay"
window = [0.0, 10.0]
opacity = 0.7
palette = "warm"
visible = false
[[layers]]
path = "data/base.nii.gz"
role = "base"
window = [1.0, 100.0]
opacity = 1.0
palette = "gray"
visible = true
"##;
        let scene: SceneFile = toml::from_str(input).unwrap();
        scene.validate().unwrap();
        let encoded = toml::to_string_pretty(&scene).unwrap();
        let decoded: SceneFile = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.layers[0].role, "overlay");
        assert_eq!(decoded.layers[1].role, "base");
        let root = std::env::temp_dir().join(format!("niv-scene-{}", std::process::id()));
        let file = root.join("session.niv.toml");
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::write(&file, input).unwrap();
        let args = vec![
            "--scene".into(),
            file.to_string_lossy().into_owned(),
            "--fps".into(),
            "12".into(),
        ];
        let c = config_from_args(&args).unwrap();
        assert_eq!(c.max_fps, 12);
        assert_eq!(c.overlays.len(), 1);
        assert_eq!(c.overlays[0].path, root.join("overlays/a.nii.gz"));
        assert_eq!(c.path, root.join("data/base.nii.gz"));
        assert_eq!(c.atlas_path, Some(root.join("atlas/labels.nii.gz")));
        assert!(c.scene.is_some());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn scene_runtime_restores_layer_order_palette_opacity_and_selection() {
        use crate::layers::IntensityOverlay;
        use crate::volume::Volume;
        use ndarray::Array3;
        let input = r#"
version = 1
[view]
axis = "y"
slice = 5
rotation = 270
stride = 3
flip_h = true
flip_v = false
selected = 0
show_overlays = false
fullscreen = false
inspect_mode = true
probe = [3,4]
[render]
background = "black"
pixel_budget = 1000000
fps = 30
hover_delay_ms = 20
mouse_pixels = false
enhanced_keys = true
[[layers]]
role = "base"
path = "base.nii.gz"
window = [10.0, 100.0]
opacity = 0.75
palette = "cool"
visible = true
[[layers]]
role = "overlay"
path = "zstat.nii.gz"
window = [2.0, 6.0]
opacity = 0.3
palette = "tint:#f700a0"
visible = false
"#;
        let scene: SceneFile = toml::from_str(input).unwrap();
        let vol = Volume {
            data: Array3::zeros((6, 7, 8)),
            spacing: [1.0; 3],
            sform: None,
            datatype: 16,
        };
        let mut app = App::new(Axis::Y, 5, 3, 270, (0., 1.), "base".into());
        let mut layers = Layers {
            overlays: vec![IntensityOverlay {
                path: PathBuf::from("zstat.nii.gz"),
                name: "zstat".into(),
                vol,
                window: (0., 1.),
                default_window: (0., 1.),
                data_range: (0., 1.),
                palette: Palette::Gray,
                visible: true,
                opacity: 1.0,
            }],
            atlas: None,
        };
        scene.apply_to_loaded(&mut app, &mut layers).unwrap();
        assert_eq!(app.layer_order, vec![1, 0]);
        assert_eq!(app.selected_layer_id(), 0);
        assert_eq!(app.window, (10.0, 100.0));
        assert_eq!(app.base_palette, Palette::Cool);
        assert_eq!(app.base_opacity, 0.75);
        assert!(app.base_visible);
        assert_eq!(app.probe, Some((3, 4)));
        assert!(app.inspect_mode);
        assert!(!app.show_overlays);
        assert!(!layers.overlays[0].visible);
        assert_eq!(layers.overlays[0].palette, Palette::Solid([247, 0, 160]));
        assert_eq!(layers.overlays[0].opacity, 0.3);
        assert!(!app.show_atlas);
        // The scene file is ordered top-to-bottom; the renderer stores bottom-to-top.
        let mut reversed = scene.clone();
        reversed.layers.reverse();
        reversed
            .layers
            .iter_mut()
            .find(|l| l.role == "base")
            .unwrap()
            .visible = false;
        reversed.view.selected = 0;
        reversed.apply_to_loaded(&mut app, &mut layers).unwrap();
        assert_eq!(app.layer_order, vec![0, 1]);
        assert_eq!(app.selected_layer_id(), 1);
        assert!(!app.base_visible);
    }

    #[test]
    fn invalid_scene_rejected_before_loading_data() {
        let mut v = SceneFile {
            version: 1,
            view: SceneView {
                axis: "x".into(),
                slice: 0,
                rotation: 90,
                stride: 1,
                flip_h: false,
                flip_v: false,
                selected: 0,
                show_overlays: true,
                fullscreen: false,
                inspect_mode: false,
                probe: None,
                montage: None,
            },
            render: SceneRender {
                background: "black".into(),
                pixel_budget: 1000,
                montage_pixel_budget: None,
                fps: 30,
                hover_delay_ms: 20,
                mouse_pixels: false,
                enhanced_keys: true,
            },
            atlas: None,
            layers: vec![SceneImage {
                path: "base.nii.gz".into(),
                role: "base".into(),
                window: [0.0, 1.0],
                opacity: 1.0,
                palette: "gray".into(),
                visible: true,
            }],
            source: None,
        };
        assert!(v.validate().is_ok());
        v.layers[0].palette = "invalid".into();
        assert!(v.validate().is_err());
        v.layers[0].palette = "gray".into();
        v.layers[0].window = [2.0, 1.0];
        assert!(v.validate().is_err());
    }

    #[test]
    fn montage_scene_backward_compatible_and_roundtrips() {
        let mut v = SceneView {
            axis: "z".into(),
            slice: 7,
            rotation: 90,
            stride: 1,
            flip_h: false,
            flip_v: false,
            selected: 0,
            show_overlays: true,
            fullscreen: false,
            inspect_mode: false,
            probe: None,
            montage: None,
        };
        let old = toml::to_string(&v).unwrap();
        assert!(toml::from_str::<SceneView>(&old).unwrap().montage.is_none());
        let m = MontageState {
            enabled: true,
            start: 2,
            stop: 36,
            interval: 4,
            columns: 3,
            gap: 6,
            rows: 2,
            show_numbers: true,
            show_clusters: true,
        };
        v.montage = Some(m.into());
        let text = toml::to_string(&v).unwrap();
        let parsed: SceneView = toml::from_str(&text).unwrap();
        assert_eq!(MontageState::from(parsed.montage.as_ref().unwrap()), m);
    }
}
