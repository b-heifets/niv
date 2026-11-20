use std::{env, path::PathBuf};

use anyhow::{bail, Context, Result};

use crate::volume::Axis;

#[derive(Clone, Debug)]
pub struct OverlaySpec {
    pub path: PathBuf,
    pub window: Option<(f32, f32)>,
    pub color: [u8; 3],
    pub grayscale: bool,
    pub opacity: f32,
}

pub struct Config {
    pub path: PathBuf,
    pub axis: Axis,
    pub initial_slice: Option<usize>,
    pub step: usize,
    pub base_opacity: f32,
    pub transparent_background: bool,
    pub hover_delay_ms: u64,
    pub atlas_opacity: f32,
    pub rotation: u16,
    pub window: Option<(f32, f32)>,
    pub overlays: Vec<OverlaySpec>,
    pub atlas_path: Option<PathBuf>,
    pub atlas_wireframe: bool,
    pub atlas_color: [u8; 3],
    pub atlas_width: usize,
    pub atlas_aa: bool,
    pub atlas_blur: f32,
    pub atlas_labels_path: Option<PathBuf>,
    pub mouse_pixels: bool,
    pub lut_path: Option<PathBuf>,
    pub max_fps: u32,
    pub enhanced_keys: bool,
    pub benchmark_frames: Option<usize>,
    pub pixel_budget: usize,
    pub profile: bool,
    /// A fully specified scene overrides environment defaults.
    pub scene: Option<crate::scene::SceneFile>,
}

const SHORT_HELP: &str = r#"niv — terminal NIfTI viewer
Usage: niv FILE/GLOB ... [OPTIONS]
       niv -i FILE/GLOB ... [OPTIONS]
       niv --scene FILE.niv.toml
Defaults: --axis x --rotate 90; $ATLAS, $ATLAS_LUT; --template uses $TEMPLATE.
Press ? in the viewer for keyboard help.  -v shows version.
Use --help for full CLI options.
"#;

const HELP: &str = r#"niv — keyboard-driven NIfTI viewer using Kitty graphics

USAGE
  niv FILE/GLOB ... [OPTIONS]
  niv -i FILE/GLOB ... [OPTIONS]
  niv --scene FILE.niv.toml [--pixel-budget N] [--fps N] [--profile]

SCENES
  s                       Save current layers + viewing settings to .niv.toml
  --scene FILE.niv.toml   Restore images, layer order, windows, palettes, atlas,
                          opacity, slice/orientation, montage, and rendering settings.
                          Relative paths resolve from the scene file folder.
                          With --scene, only --pixel-budget, --fps, --profile,
                          and --bench may be supplied as overrides.

VIEW
  -a, --axis x|y|z       Slice axis (default x; NIV_AXIS or NII_AXIS)
  -r, --rotate DEGREES  Clockwise 0,90,180,270 (default 90; NIV_ROTATE)
      --slice N          Starting slice (zero based; default center)
      --step N           Starting slice stride (default 1)
  -w, --window MIN,MAX  Initial display range (default finite voxel min,max)
      --opacity 0..1    Base image opacity (default 1)
      --background black|transparent  Kitty pixel format (default black/RGB;
                       NIV_BACKGROUND may set the default; b toggles in TUI)
      --hover-delay-ms N  Idle milliseconds before highlighting a hovered atlas
                       region (default 20ms; NIV_HOVER_DELAY_MS; 0 disables highlight)
      --fps N            Maximum redraw FPS (default 30; NIV_FPS)
      --pixel-budget N   Max output raster pixels (default 1000000; 0 = native)
                       NIV_PIXEL_BUDGET sets the default. Does not affect
                       the full voxel data loaded or the displayed image area.
      --profile          Report raster/encode/output timings on exit
      --mouse-pixels     Request high-precision SGR 1016 mouse events
      --no-enhanced-keys Disable Kitty enhanced keyboard mode
      --bench N          Benchmark N frames without a terminal

LAYERS AND ATLAS
  -i, --images FILE/GLOB ...      Load NIfTI images (no positional file required).
                                   Supports multiple space-separated files,
                                   quoted globs, and repeated -i groups.
                                   Matched files are sorted; all images are
                                   aligned to the base volume. Default grayscale.
      --template                  Append $TEMPLATE as the bottom grayscale layer
      --overlay FILE              Additional image with fixed warm tint; repeatable
      --overlay-window MIN,MAX    Most recently supplied overlay range
      --overlay-color RRGGBB      Most recent overlay fixed tint
      --overlay-opacity 0..1      Most recent overlay opacity (default 1)
      --atlas FILE                Solid ID-label atlas; default $ATLAS
      --atlas-wireframe FILE      Precomputed ID/binary wireframe; overrides $ATLAS
      --atlas-labels FILE         Companion solid atlas for wireframe inspection
      --lut FILE.csv              LUT; default $ATLAS_LUT when atlas is present
      --atlas-color RRGGBB        Fallback contour color (default white)
      --atlas-opacity 0..1        Atlas opacity (default 1)
      --atlas-width N             Contour width, 1–3 pixels (default 1)
      --atlas-blur SIGMA          Gaussian blur of atlas strokes only, 0..4 px
                                  (NIV_ATLAS_BLUR; default 0; G edits)
      --atlas-aa                  Optional subpixel antialiasing of solid-atlas outlines
                                  (NIV_ATLAS_AA=1; toggle with g; slower than exact)

INTERACTIVE HELP
  Press ? in the viewer for context-specific keyboard controls.
  -v, -V, --version       Show version.

EXAMPLES
  niv mean_IF.nii.gz
  niv template.nii.gz clusters.nii.gz
  niv -i template.nii.gz "*clusters*.nii.gz"
  ATLAS=/data/atlas.nii.gz ATLAS_LUT=/data/CCFv3-2020_regional_summary.csv niv mean_IF.nii.gz
  niv mean_IF.nii.gz -i 'stat*.nii.gz' '*clusters*.nii.gz'
  niv mean_IF.nii.gz -i layer1.nii.gz layer2.nii.gz --atlas atlas.nii.gz
  niv mean_IF.nii.gz --atlas other.nii.gz --overlay zstat.nii.gz --overlay-window 2,6

Atlas and intensity overlays require identical voxel grids; no affine reorientation
or resampling is performed. Image-layer threshold transparency and opacity
work in both modes; transparent terminal background requires Kitty RGBA (f=32).
"#;

pub fn parse() -> Result<Option<Config>> {
    parse_args(env::args().skip(1).collect())
}

fn parse_args(argv: Vec<String>) -> Result<Option<Config>> {
    parse_args_with_template(argv, env::var_os("TEMPLATE"))
}

fn parse_args_with_template(
    argv: Vec<String>,
    template_env: Option<std::ffi::OsString>,
) -> Result<Option<Config>> {
    // Help/version can be requested even when --scene is also on the command line.
    if argv.iter().any(|a| a == "--help") {
        print!("{HELP}");
        return Ok(None);
    }
    if argv.iter().any(|a| a == "-h") {
        print!("{SHORT_HELP}");
        return Ok(None);
    }
    if argv
        .iter()
        .any(|a| a == "--version" || a == "-V" || a == "-v")
    {
        println!("niv {}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }
    // A saved scene is a self-contained input mode. Accept only resource/perf
    // overrides alongside --scene so layer options cannot silently conflict.
    if argv
        .iter()
        .any(|a| a == "--scene" || a.starts_with("--scene="))
    {
        return Ok(Some(crate::scene::config_from_args(&argv)?));
    }
    let mut args = argv.into_iter().peekable();
    let mut axis = Axis::parse(
        &env::var("NIV_AXIS")
            .or_else(|_| env::var("NII_AXIS"))
            .unwrap_or_else(|_| "x".to_owned()),
    )?;
    let mut rotation = parse_rotation(&env::var("NIV_ROTATE").unwrap_or_else(|_| "90".to_owned()))?;
    let mut path = None;
    let mut use_template = false;
    let mut initial_slice = None;
    let mut step = 1;
    let mut window = None;
    let mut base_opacity = 1.0f32;
    let mut transparent_background =
        parse_background(&env::var("NIV_BACKGROUND").unwrap_or_else(|_| "black".to_owned()))?;
    let mut hover_delay_ms = env::var("NIV_HOVER_DELAY_MS")
        .ok()
        .map(|v| parse_delay(&v))
        .transpose()?
        .unwrap_or(20);
    let mut atlas_opacity = 1.0f32;
    let mut overlays: Vec<OverlaySpec> = Vec::new();
    let mut atlas_path = None;
    let mut atlas_wireframe = false;
    let mut atlas_color = [255, 255, 255];
    let mut atlas_width = 1;
    let mut atlas_blur = env::var("NIV_ATLAS_BLUR")
        .ok()
        .map(|s| parse_blur(&s))
        .transpose()?
        .unwrap_or(0.);
    let mut atlas_aa = matches!(
        env::var("NIV_ATLAS_AA").ok().as_deref(),
        Some("1" | "true" | "yes")
    );
    let mut lut_path = None;
    let mut atlas_labels_path = None;
    let mut mouse_pixels = false;
    let mut max_fps = env::var("NIV_FPS")
        .ok()
        .map(|s| parse_range(&s, "NIV_FPS", 1, 120))
        .transpose()?
        .unwrap_or(30) as u32;
    let mut pixel_budget = env::var("NIV_PIXEL_BUDGET")
        .ok()
        .map(|s| parse_pixel_budget(&s))
        .transpose()?
        .unwrap_or(1_000_000);
    let mut profile = false;
    let mut enhanced_keys = true;
    let mut benchmark_frames = None;
    while let Some(arg) = args.next() {
        if arg == "-h" {
            print!("{SHORT_HELP}");
            return Ok(None);
        }
        if arg == "--help" {
            print!("{HELP}");
            return Ok(None);
        }
        if arg == "-V" || arg == "-v" || arg == "--version" {
            println!("niv {}", env!("CARGO_PKG_VERSION"));
            return Ok(None);
        }
        if arg == "--profile" {
            profile = true;
            continue;
        }
        if arg == "--template" {
            use_template = true;
            continue;
        }
        if arg == "--atlas-aa" {
            atlas_aa = true;
            continue;
        }
        if arg == "--mouse-pixels" {
            mouse_pixels = true;
            continue;
        }
        if arg == "--no-enhanced-keys" {
            enhanced_keys = false;
            continue;
        }
        // -i consumes filenames/globs until the next flag, then expands each
        // pattern in place. Quoted globs work even if zsh does not expand them.
        // Keep the legacy --overlay flag (with its own tint/window options).
        if arg == "-i" || arg == "--images" || arg.starts_with("--images=") {
            let mut patterns = Vec::new();
            if let Some((_, v)) = arg.split_once('=') {
                patterns.push(v.to_owned());
            }
            while let Some(next) = args.peek() {
                if next.starts_with('-') {
                    break;
                }
                patterns.push(args.next().expect("peeked"));
            }
            if patterns.is_empty() || patterns.iter().any(|p| p.is_empty()) {
                bail!("-i/--images requires at least one image path or glob");
            }
            for pattern in patterns {
                for found in expand_images(&pattern)? {
                    append_image(&mut path, &mut overlays, found);
                }
            }
            continue;
        }
        // Handle both "--option value" and "--option=value" forms.
        let (flag, inline_value) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
        let needs_value = matches!(
            flag,
            "-a" | "--axis"
                | "--slice"
                | "--step"
                | "-w"
                | "--window"
                | "-r"
                | "--rotate"
                | "--fps"
                | "--bench"
                | "--overlay"
                | "--overlay-window"
                | "--overlay-color"
                | "--overlay-opacity"
                | "--atlas"
                | "--atlas-wireframe"
                | "--atlas-color"
                | "--atlas-width"
                | "--atlas-blur"
                | "--lut"
                | "--atlas-labels"
                | "--atlas-opacity"
                | "--opacity"
                | "--background"
                | "--hover-delay-ms"
                | "--pixel-budget"
        );
        if needs_value {
            let val = if let Some(v) = inline_value {
                v.to_owned()
            } else {
                args.next()
                    .with_context(|| format!("{flag} needs a value"))?
            };
            match flag {
                "-a" | "--axis" => axis = Axis::parse(&val)?,
                "--slice" => {
                    initial_slice = Some(
                        val.parse()
                            .context("--slice requires a nonnegative integer")?,
                    )
                }
                "--step" => step = parse_range(&val, flag, 1, 256)?,
                "-r" | "--rotate" => rotation = parse_rotation(&val)?,
                "-w" | "--window" => window = Some(parse_window(&val)?),
                "--opacity" => base_opacity = parse_opacity(&val)?,
                "--background" => transparent_background = parse_background(&val)?,
                "--hover-delay-ms" => hover_delay_ms = parse_delay(&val)?,
                "--atlas-opacity" => atlas_opacity = parse_opacity(&val)?,
                "--overlay" => overlays.push(OverlaySpec {
                    path: PathBuf::from(val),
                    window: None,
                    color: [255, 184, 77],
                    grayscale: false,
                    opacity: 1.0,
                }),
                "--overlay-window" => {
                    overlays
                        .last_mut()
                        .context("--overlay-window requires a preceding --overlay")?
                        .window = Some(parse_window(&val)?);
                }
                "--overlay-color" => {
                    let overlay = overlays
                        .last_mut()
                        .context("--overlay-color requires a preceding image")?;
                    overlay.color = parse_color(&val)?;
                    overlay.grayscale = false;
                }
                "--overlay-opacity" => {
                    overlays
                        .last_mut()
                        .context("--overlay-opacity requires a preceding --overlay")?
                        .opacity = parse_opacity(&val)?;
                }
                "--atlas" | "--atlas-wireframe" => {
                    if atlas_path.is_some() {
                        bail!("Use only one of --atlas / --atlas-wireframe");
                    }
                    atlas_wireframe = flag == "--atlas-wireframe";
                    atlas_path = Some(PathBuf::from(val));
                }
                "--atlas-color" => atlas_color = parse_color(&val)?,
                "--atlas-blur" => atlas_blur = parse_blur(&val)?,
                "--atlas-width" => atlas_width = parse_range(&val, flag, 1, 3)?,
                "--lut" => lut_path = Some(PathBuf::from(val)),
                "--atlas-labels" => atlas_labels_path = Some(PathBuf::from(val)),
                "--fps" => max_fps = parse_range(&val, flag, 1, 120)? as u32,
                "--pixel-budget" => pixel_budget = parse_pixel_budget(&val)?,
                "--bench" => benchmark_frames = Some(parse_range(&val, flag, 1, 10_000)?),
                _ => unreachable!(),
            }
        } else if arg.starts_with('-') {
            bail!("Unknown option {arg:?}. See --help");
        } else {
            for found in expand_images(&arg)? {
                append_image(&mut path, &mut overlays, found);
            }
        }
    }
    // Explicit opt-in: no extra template loading during normal startup.
    if use_template {
        let template = template_env
            .filter(|v| !v.is_empty())
            .context("--template requires the TEMPLATE environment variable")?;
        append_image(&mut path, &mut overlays, PathBuf::from(template));
    }
    // Explicit --atlas/--atlas-wireframe takes precedence over environment defaults.
    if atlas_path.is_none() {
        atlas_path = env::var_os("ATLAS")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    // A global LUT setting should not prevent running without an atlas.
    if atlas_path.is_some() && lut_path.is_none() {
        lut_path = env::var_os("ATLAS_LUT")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    if atlas_labels_path.is_some() && (!atlas_wireframe || atlas_path.is_none()) {
        bail!("--atlas-labels is only used with --atlas-wireframe");
    }
    Ok(Some(Config {
        path: path.context("Missing NIfTI file. See: niv --help")?,
        axis,
        initial_slice,
        step,
        rotation,
        window,
        base_opacity,
        atlas_opacity,
        transparent_background,
        hover_delay_ms,
        overlays,
        atlas_path,
        atlas_wireframe,
        atlas_color,
        atlas_width,
        atlas_aa,
        atlas_blur,
        lut_path,
        atlas_labels_path,
        mouse_pixels,
        max_fps,
        enhanced_keys,
        benchmark_frames,
        pixel_budget,
        profile,
        scene: None,
    }))
}

/// First ordinary image is the reference; later ones are grayscale layers.
fn append_image(base: &mut Option<PathBuf>, overlays: &mut Vec<OverlaySpec>, path: PathBuf) {
    if base.is_none() {
        *base = Some(path);
        return;
    }
    if base.as_ref() == Some(&path) || overlays.iter().any(|s| s.path == path) {
        return;
    }
    overlays.push(OverlaySpec {
        path,
        window: None,
        color: [255, 184, 77],
        grayscale: true,
        opacity: 1.0,
    });
}

/// Expand a shell-style glob without relying on the user's zsh glob settings.
/// Non-glob paths are kept for the normal NIfTI loader to report any errors.
fn expand_images(input: &str) -> Result<Vec<PathBuf>> {
    let input_path = crate::path_complete::resolve(input);
    if input_path.is_file() {
        return Ok(vec![input_path]);
    }
    let pattern = input_path.to_string_lossy().into_owned();
    if !(pattern.contains('*') || pattern.contains('?') || pattern.contains('[')) {
        return Ok(vec![input_path]);
    }
    let mut found = Vec::new();
    for entry in glob::glob(&pattern).with_context(|| format!("Invalid image glob {input:?}"))? {
        let path = entry.with_context(|| format!("Expanding image glob {input:?}"))?;
        if path.is_file()
            && (path
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".nii")
                || path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with(".nii.gz"))
        {
            found.push(path);
        }
    }
    found.sort();
    found.dedup();
    if found.is_empty() {
        bail!("No NIfTI files match glob {input:?}");
    }
    Ok(found)
}

fn parse_pixel_budget(s: &str) -> Result<usize> {
    let n = s
        .parse::<usize>()
        .context("Pixel budget must be an integer (0..10000000)")?;
    if n > 10_000_000 {
        bail!("Pixel budget must be between 0 and 10000000");
    }
    Ok(n)
}

fn parse_background(s: &str) -> Result<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "black" | "rgb" => Ok(false),
        "transparent" | "rgba" => Ok(true),
        _ => bail!("Background must be black or transparent"),
    }
}

fn parse_delay(s: &str) -> Result<u64> {
    let n: u64 = s
        .parse()
        .context("Hover delay must be an integer (0..2000 ms)")?;
    if n > 2000 {
        bail!("Hover delay must be between 0 and 2000 ms");
    }
    Ok(n)
}

fn parse_blur(s: &str) -> Result<f32> {
    let sigma: f32 = s.parse().context("Invalid atlas blur")?;
    if !sigma.is_finite() || !(0.0..=4.0).contains(&sigma) {
        bail!("Atlas blur sigma must be 0..4 px");
    }
    Ok(sigma)
}

fn parse_opacity(s: &str) -> Result<f32> {
    let x: f32 = s.parse().context("Opacity must be a number in [0, 1]")?;
    if !x.is_finite() || !(0.0..=1.0).contains(&x) {
        bail!("Opacity must be in [0, 1]");
    }
    Ok(x)
}

fn parse_color(s: &str) -> Result<[u8; 3]> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        bail!("Color must be a 6-digit RGB hex value, e.g. ff0000");
    }
    Ok([
        u8::from_str_radix(&s[0..2], 16).context("Invalid color red hex pair")?,
        u8::from_str_radix(&s[2..4], 16).context("Invalid color green hex pair")?,
        u8::from_str_radix(&s[4..6], 16).context("Invalid color blue hex pair")?,
    ])
}

fn parse_range(s: &str, flag: &str, low: usize, high: usize) -> Result<usize> {
    let value: usize = s
        .parse()
        .with_context(|| format!("{flag} must be an integer"))?;
    if !(low..=high).contains(&value) {
        bail!("{flag} must be between {low} and {high}");
    }
    Ok(value)
}

fn parse_rotation(s: &str) -> Result<u16> {
    match s {
        "0" => Ok(0),
        "90" => Ok(90),
        "180" => Ok(180),
        "270" => Ok(270),
        _ => bail!("Rotation must be 0, 90, 180, or 270 degrees"),
    }
}

fn parse_window(s: &str) -> Result<(f32, f32)> {
    let (a, b) = s
        .split_once(',')
        .context("Window format must be LOW,HIGH")?;
    let lo: f32 = a.trim().parse().context("Invalid window LOW")?;
    let hi: f32 = b.trim().parse().context("Invalid window HIGH")?;
    if !lo.is_finite() || !hi.is_finite() || lo >= hi {
        bail!("Window requires finite LOW < HIGH");
    }
    Ok((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positional_and_i_only_loading_preserve_order() {
        for args in [
            vec!["a.nii.gz", "b.nii.gz"],
            vec!["-i", "a.nii.gz", "b.nii.gz"],
            vec!["a.nii.gz", "-i", "b.nii.gz"],
            vec!["-i", "a.nii.gz", "-i", "b.nii.gz", "a.nii.gz"],
        ] {
            let cfg = parse_args(args.into_iter().map(str::to_owned).collect())
                .unwrap()
                .unwrap();
            assert_eq!(cfg.path, PathBuf::from("a.nii.gz"));
            assert_eq!(cfg.overlays.len(), 1);
            assert_eq!(cfg.overlays[0].path, PathBuf::from("b.nii.gz"));
            assert!(cfg.overlays[0].grayscale);
            assert_eq!(cfg.overlays[0].opacity, 1.0);
        }
    }
    #[test]
    fn template_is_opt_in_and_appended_to_bottom() {
        let args = |values: &[&str]| values.iter().map(|s| s.to_string()).collect();
        let template = || Some(std::ffi::OsString::from("template.nii.gz"));
        let cfg = parse_args_with_template(args(&["a.nii.gz"]), template())
            .unwrap()
            .unwrap();
        assert!(cfg.overlays.is_empty());
        let cfg =
            parse_args_with_template(args(&["--template", "a.nii.gz", "b.nii.gz"]), template())
                .unwrap()
                .unwrap();
        assert_eq!(cfg.path, PathBuf::from("a.nii.gz"));
        assert_eq!(cfg.overlays.len(), 2);
        assert_eq!(cfg.overlays[1].path, PathBuf::from("template.nii.gz"));
        assert!(cfg.overlays[1].grayscale);
        assert_eq!(cfg.overlays[1].opacity, 1.0);
        assert!(parse_args_with_template(args(&["a.nii.gz", "--template"]), None).is_err());
        let cfg = parse_args_with_template(args(&["--template"]), template())
            .unwrap()
            .unwrap();
        assert_eq!(cfg.path, PathBuf::from("template.nii.gz"));
    }
    #[test]
    fn multiple_glob_patterns_expand_in_sorted_order() {
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };
        let root = std::env::temp_dir().join(format!(
            "niv-glob-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        for name in ["x_a.nii.gz", "x_b.nii.gz", "y_1.nii", "ignore.csv"] {
            fs::write(root.join(name), b"").unwrap();
        }
        let files = expand_images(&format!("{}/x_*.nii.gz", root.display())).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("x_a.nii.gz"));
        assert_eq!(
            expand_images(&format!("{}/y_?.nii", root.display()))
                .unwrap()
                .len(),
            1
        );
        let cfg = parse_args(vec![
            "-i".into(),
            format!("{}/x_*.nii.gz", root.display()),
            format!("{}/y_?.nii", root.display()),
        ])
        .unwrap()
        .unwrap();
        assert!(cfg.path.ends_with("x_a.nii.gz"));
        assert_eq!(cfg.overlays.len(), 2);
        assert!(cfg.overlays[0].path.ends_with("x_b.nii.gz"));
        assert!(cfg.overlays[1].path.ends_with("y_1.nii"));
        assert!(expand_images(&format!("{}/z*.nii.gz", root.display())).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn test_window() {
        assert_eq!(parse_window("-5, 10").unwrap(), (-5., 10.));
        assert!(parse_window("5,5").is_err());
        assert!(parse_window("NaN,2").is_err());
    }
    #[test]
    fn test_color_and_opacity() {
        assert_eq!(parse_color("#ff0080").unwrap(), [255, 0, 128]);
        assert!(parse_color("white").is_err());
        assert_eq!(parse_opacity(".5").unwrap(), 0.5);
        assert!(parse_opacity("1.5").is_err());
    }
    #[test]
    fn pixel_budget_limits() {
        assert_eq!(parse_pixel_budget("400000").unwrap(), 400000);
        assert_eq!(parse_pixel_budget("0").unwrap(), 0);
        assert!(parse_pixel_budget("10000001").is_err());
    }
    #[test]
    fn background_and_delay() {
        assert!(!parse_background("black").unwrap());
        assert!(parse_background("RGBA").unwrap());
        assert!(parse_background("transParent").unwrap());
        assert!(parse_background("other").is_err());
        assert_eq!(parse_delay("150").unwrap(), 150);
        assert_eq!(parse_delay("0").unwrap(), 0);
        assert!(parse_delay("2001").is_err());
    }
    #[test]
    fn test_rotation() {
        assert_eq!(parse_rotation("90").unwrap(), 90);
        assert!(parse_rotation("45").is_err());
        assert!(parse_rotation("-90").is_err());
    }
    #[test]
    fn test_range() {
        assert_eq!(parse_range("30", "--fps", 1, 120).unwrap(), 30);
        assert!(parse_range("0", "--fps", 1, 120).is_err());
    }
}
