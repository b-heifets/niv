use std::{env, path::PathBuf};

use anyhow::{bail, Context, Result};

use crate::volume::Axis;

#[derive(Clone, Debug)]
pub struct OverlaySpec {
    pub path: PathBuf,
    pub window: Option<(f32, f32)>,
    pub color: [u8; 3],
    pub opacity: f32,
}

pub struct Config {
    pub path: PathBuf,
    pub axis: Axis,
    pub initial_slice: Option<usize>,
    pub step: usize,
    pub rotation: u16,
    pub window: Option<(f32, f32)>,
    pub overlays: Vec<OverlaySpec>,
    pub atlas_path: Option<PathBuf>,
    pub atlas_wireframe: bool,
    pub atlas_color: [u8; 3],
    pub atlas_width: usize,
    pub atlas_labels_path: Option<PathBuf>,
    pub mouse_pixels: bool,
    pub lut_path: Option<PathBuf>,
    pub max_fps: u32,
    pub enhanced_keys: bool,
    pub benchmark_frames: Option<usize>,
}

const HELP: &str = r#"niv — fast NIfTI slice viewer using Kitty graphics

USAGE
  niv FILE.nii[.gz] [OPTIONS]

OPTIONS
  -a, --axis x|y|z       Voxel index axis for slicing (NIV_AXIS or NII_AXIS; default z)
      --slice N          Start at zero-based slice N (default: center)
      --step N           Base slice stride (default: 1)
  -r, --rotate DEGREES  Rotate view clockwise: 0,90,180,270 (NIV_ROTATE; default 0)
  -w, --window LOW,HIGH  Grayscale window (default: sampled nonzero 1%-99.5%)
      --overlay FILE     Add an intensity layer (repeatable, drawn in CLI order)
      --overlay-window LOW,HIGH  Window for most recently added overlay
      --overlay-color HEX  Tint for most recent overlay (default #ffb84d)
      --overlay-opacity 0..1  Opacity for most recent overlay (default .8)
      --atlas FILE       Solid ID label atlas; draw between-voxel contours
      --atlas-wireframe FILE  Precomputed binary or ID wireframe volume
      --lut FILE.csv     CCFv3 table with Region_ID,Region,Abbr,R,G,B
      --atlas-color HEX Fallback outline color (#ffffff)
      --atlas-width N    Outline thickness in pixels, 1..3 (default 1)
      --atlas-labels FILE  Solid label volume for region inspection with wireframe
      --mouse-pixels     Request SGR 1016 pixel-precision mouse events (opt-in)
      --fps N            Maximum redraw FPS (default: 30; useful over Tailscale)
      --no-enhanced-keys Disable Kitty enhanced keyboard mode (for problematic muxes)
      --bench N          Render and encode N frames, report timings, then exit
  -h, --help             Show help
  -V, --version          Show version

KEYS
  Up/Right or k/l       Next slice (positive voxel index)
  Down/Left or j/h      Previous slice
  Shift+arrows 2x; Ctrl+arrows 3x; Alt+arrows 4x
  Mouse wheel / two-finger trackpad scroll: change slices
  [ and ]               Decrease/increase stride by 1 (1..256)
  PgUp / PgDn           +/-25 slices; Home / End first/last
  x / y / z             Pick slicing axis
  r / R                 Rotate view clockwise / counterclockwise 90 degrees
  f / v                 Flip horizontal / vertical view
  + / -                 Narrow / widen grayscale window (contrast)
  , / .                 Shift window down / up (brightness)
  0                     Reset selected layer window
  a                     Toggle atlas outlines
  Tab                   Toggle image-only fullscreen
  t                     Show/hide layer list
  1..9                  Select intensity layer (1 = base)
  e                     Type selected layer window as LOW,HIGH
  m / M                 Edit selected layer minimum / maximum
  c                     Cycle grayscale / warm / cool / saturated palette
  Space                 Show/hide selected intensity overlay
  i                     Toggle keyboard voxel inspector; move with arrows/hjkl
  Mouse hover           Inspect voxel (if terminal/Rex forwards mouse motion)
  o                     Toggle all additional intensity overlays
  q / Esc / Ctrl+C      Quit

EXAMPLES
  niv brain.nii.gz
  NIV_AXIS=y niv /path/to/brain.nii.gz --fps 12
  niv brain.nii.gz -a x --rotate 90 --window 0,3000 --step 2
  niv brain.nii.gz --atlas atlas.nii.gz --lut CCFv3-2020_regional_summary.csv
  niv brain.nii.gz --overlay zstat.nii.gz --overlay-window 2,6 --overlay-color ff5555
  niv brain.nii.gz --atlas-wireframe atlas_W_IDs.nii.gz --lut CCFv3-2020_regional_summary.csv
  niv brain.nii.gz --bench 50

Axes are NIfTI ARRAY INDICES, not anatomical RAS/LPS axes. Physical aspect ratio
uses pixdim, but the viewer does not apply the sform/qform affine yet.
"#;

pub fn parse() -> Result<Option<Config>> {
    let mut args = env::args().skip(1);
    let mut axis = Axis::parse(
        &env::var("NIV_AXIS")
            .or_else(|_| env::var("NII_AXIS"))
            .unwrap_or_else(|_| "z".to_owned()),
    )?;
    let mut rotation = parse_rotation(&env::var("NIV_ROTATE").unwrap_or_else(|_| "0".to_owned()))?;
    let mut path = None;
    let mut initial_slice = None;
    let mut step = 1;
    let mut window = None;
    let mut overlays: Vec<OverlaySpec> = Vec::new();
    let mut atlas_path = None;
    let mut atlas_wireframe = false;
    let mut atlas_color = [255, 255, 255];
    let mut atlas_width = 1;
    let mut lut_path = None;
    let mut atlas_labels_path = None;
    let mut mouse_pixels = false;
    let mut max_fps = 30;
    let mut enhanced_keys = true;
    let mut benchmark_frames = None;
    while let Some(arg) = args.next() {
        if arg == "-h" || arg == "--help" {
            print!("{HELP}");
            return Ok(None);
        }
        if arg == "-V" || arg == "--version" {
            println!("niv {}", env!("CARGO_PKG_VERSION"));
            return Ok(None);
        }
        if arg == "--mouse-pixels" {
            mouse_pixels = true;
            continue;
        }
        if arg == "--no-enhanced-keys" {
            enhanced_keys = false;
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
                | "--lut"
                | "--atlas-labels"
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
                "--overlay" => overlays.push(OverlaySpec {
                    path: PathBuf::from(val),
                    window: None,
                    color: [255, 184, 77],
                    opacity: 0.8,
                }),
                "--overlay-window" => {
                    overlays
                        .last_mut()
                        .context("--overlay-window requires a preceding --overlay")?
                        .window = Some(parse_window(&val)?);
                }
                "--overlay-color" => {
                    overlays
                        .last_mut()
                        .context("--overlay-color requires a preceding --overlay")?
                        .color = parse_color(&val)?;
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
                "--atlas-width" => atlas_width = parse_range(&val, flag, 1, 3)?,
                "--lut" => lut_path = Some(PathBuf::from(val)),
                "--atlas-labels" => atlas_labels_path = Some(PathBuf::from(val)),
                "--fps" => max_fps = parse_range(&val, flag, 1, 120)? as u32,
                "--bench" => benchmark_frames = Some(parse_range(&val, flag, 1, 10_000)?),
                _ => unreachable!(),
            }
        } else if arg.starts_with('-') {
            bail!("Unknown option {arg:?}. See --help");
        } else if path.replace(PathBuf::from(arg)).is_some() {
            bail!("Supply exactly one NIfTI file");
        }
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
        overlays,
        atlas_path,
        atlas_wireframe,
        atlas_color,
        atlas_width,
        lut_path,
        atlas_labels_path,
        mouse_pixels,
        max_fps,
        enhanced_keys,
        benchmark_frames,
    }))
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
