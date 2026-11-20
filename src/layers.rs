//! Voxel-aligned intensity overlays and colored atlas outlines.
//! On-the-fly outlines are rasterized at voxel FACE coordinates, not at centers.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::{
    config::Config,
    volume::{Axis, Volume},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Gray,
    Warm,
    Cool,
    Random,
    Solid([u8; 3]),
}
impl Palette {
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            Self::Gray => "gray",
            Self::Warm => "warm",
            Self::Cool => "cool",
            Self::Random => "categorical",
            Self::Solid(_) => "tint",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Gray => Self::Warm,
            Self::Warm => Self::Cool,
            Self::Cool => Self::Random,
            Self::Random => Self::Gray,
            Self::Solid(_) => Self::Gray,
        }
    }
    pub fn rgb(self, level: f32, layer: usize) -> [u8; 3] {
        let t = level.clamp(0., 1.);
        let cv = |v: f32| -> u8 { (v.clamp(0., 1.) * 255.).round() as u8 };
        match self {
            Self::Gray => {
                let g = cv(t);
                [g, g, g]
            }
            Self::Warm => [255, cv(t), 0],
            Self::Cool => [0, cv(t), 255],
            Self::Random => categorical_color(layer as i32),
            Self::Solid(rgb) => rgb,
        }
    }
    /// Continuous palettes use window-normalized brightness; categorical colors
    /// depend ONLY on the integer voxel value, not on the layer number.
    pub fn voxel_color(self, value: f32, lo: f32, hi: f32, layer: usize) -> [u8; 3] {
        if self == Self::Random {
            categorical_color(value.round() as i32)
        } else {
            self.rgb(((value - lo) / (hi - lo)).clamp(0., 1.), layer)
        }
    }
    pub fn accent(self, layer: usize) -> [u8; 3] {
        match self {
            Self::Gray => [210; 3],
            other => other.rgb(1., layer),
        }
    }
}

/// Deterministic categorical LUT: neighboring integer IDs get well-separated hues.
/// A second integer hash jitters saturation/value to distinguish many (up to 255+)
/// categories without changing colors between slices or layers.
fn categorical_color(id: i32) -> [u8; 3] {
    let key = id as u32;
    let mix = (key ^ 0x9e37_79b9)
        .wrapping_mul(0x85eb_ca6b)
        .rotate_left(13);
    let hue = (id as f64 * 0.618_033_988_749_895 + 0.137).rem_euclid(1.0);
    let saturation = 0.80 + (mix & 7) as f64 / 70.0;
    let value = 0.85 + ((mix >> 4) & 7) as f64 / 50.0;
    hsv_to_rgb(hue, saturation, value.min(1.0))
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [u8; 3] {
    let hue = (h.rem_euclid(1.0) * 6.0).clamp(0.0, 5.999_999);
    let c = v * s;
    let x = c * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    ]
}

/// Only inspect the filename, not its parent path. CI is a separate token.
pub fn default_palette(path: &Path) -> Palette {
    let name = short_name(path).to_ascii_lowercase();
    if name.contains("cluster")
        || name
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "ci")
    {
        Palette::Random
    } else {
        Palette::Gray
    }
}

pub fn short_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    name.strip_suffix(".nii.gz")
        .or_else(|| name.strip_suffix(".nii"))
        .unwrap_or(&name)
        .to_owned()
}

pub struct IntensityOverlay {
    pub path: PathBuf,
    pub name: String,
    pub vol: Volume,
    pub window: (f32, f32),
    pub default_window: (f32, f32),
    pub data_range: (f32, f32),
    pub palette: Palette,
    pub visible: bool,
    pub opacity: f32,
}

#[derive(Clone)]
pub struct RegionInfo {
    pub color: [u8; 3],
    pub name: String,
    pub abbr: String,
    pub general_region: String,
}

pub struct Atlas {
    pub vol: Volume,
    pub wireframe: bool,
    pub labels: Option<Volume>,
    pub fallback: [u8; 3],
    pub width: usize,
    pub opacity: f32,
    pub regions: HashMap<i32, RegionInfo>,
}

pub struct Layers {
    pub overlays: Vec<IntensityOverlay>,
    pub atlas: Option<Atlas>,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            overlays: Vec::new(),
            atlas: None,
        }
    }
}

impl Layers {
    pub fn load(config: &Config, reference: &Volume) -> Result<Self> {
        let mut overlays = Vec::new();
        for spec in &config.overlays {
            let vol = Volume::load(&spec.path)
                .with_context(|| format!("Loading intensity overlay {}", spec.path.display()))?;
            reference.validate_grid(&vol).with_context(|| {
                format!(
                    "Intensity overlay {} is not voxel-aligned",
                    spec.path.display()
                )
            })?;
            let data_range = vol.data_range();
            let default_window = if data_range.0 < data_range.1 {
                data_range
            } else {
                (data_range.0 - 0.5, data_range.1 + 0.5)
            };
            let window = spec.window.unwrap_or(default_window);
            let name = short_name(&spec.path);
            overlays.push(IntensityOverlay {
                path: spec.path.clone(),
                vol,
                window,
                default_window,
                data_range,
                palette: if spec.grayscale {
                    default_palette(&spec.path)
                } else {
                    Palette::Solid(spec.color)
                },
                visible: true,
                opacity: spec.opacity,
                name,
            });
        }
        let atlas = if let Some(path) = &config.atlas_path {
            Some(load_atlas(
                path,
                reference,
                config.atlas_wireframe,
                config.atlas_labels_path.as_deref(),
                config.lut_path.as_deref(),
                config.atlas_color,
                config.atlas_width,
                config.atlas_opacity,
            )?)
        } else {
            if config.lut_path.is_some() {
                bail!("--lut requires --atlas or --atlas-wireframe");
            }
            None
        };
        Ok(Self { overlays, atlas })
    }

    /// Load another aligned image without changing any existing layer.
    pub fn add_overlay(&mut self, path: &Path, reference: &Volume) -> Result<usize> {
        let vol = Volume::load(path).with_context(|| format!("Loading {}", path.display()))?;
        reference
            .validate_grid(&vol)
            .with_context(|| format!("{} is not voxel-aligned", path.display()))?;
        let data_range = vol.data_range();
        let window = if data_range.0 < data_range.1 {
            data_range
        } else {
            (data_range.0 - 0.5, data_range.1 + 0.5)
        };
        self.overlays.push(IntensityOverlay {
            path: path.to_path_buf(),
            name: short_name(path),
            vol,
            window,
            default_window: window,
            data_range,
            palette: default_palette(path),
            visible: true,
            opacity: 1.0,
        });
        Ok(self.overlays.len()) // index 0 = base; index 1 = first overlay
    }

    /// Full region text at a voxel, or None for unlabeled voxels. For a
    /// precomputed wireframe a separate solid atlas is required for reliable
    /// region lookup away from outline voxels.
    #[cfg(test)]
    pub fn region_at(&self, axis: Axis, slice: usize, h: usize, v: usize) -> Option<String> {
        let atlas = self.atlas.as_ref()?;
        let labels = if atlas.wireframe {
            atlas.labels.as_ref()?
        } else {
            &atlas.vol
        };
        let id = label_id(labels.sample(axis, slice, h, v));
        if id == 0 {
            return Some("outside atlas".to_string());
        }
        if let Some(reg) = atlas.regions.get(&id) {
            Some(format!("{} — {} (ID {})", reg.abbr, reg.name, id))
        } else {
            Some(format!("region ID {} (not in LUT)", id))
        }
    }

    /// Region label at the cursor, even when a precomputed outline needs a
    /// separate solid label volume. Unlabeled background is None.
    pub fn atlas_id_at(&self, axis: Axis, slice: usize, h: usize, v: usize) -> Option<i32> {
        let atlas = self.atlas.as_ref()?;
        let labels = if atlas.wireframe {
            atlas.labels.as_ref()?
        } else {
            &atlas.vol
        };
        let id = label_id(labels.sample(axis, slice, h, v));
        if id > 0 {
            Some(id)
        } else {
            None
        }
    }

    pub fn region_details(
        &self,
        axis: Axis,
        slice: usize,
        h: usize,
        v: usize,
    ) -> Option<(String, Option<[u8; 3]>)> {
        let atlas = self.atlas.as_ref()?;
        let labels = if atlas.wireframe {
            atlas.labels.as_ref()?
        } else {
            &atlas.vol
        };
        let id = label_id(labels.sample(axis, slice, h, v));
        if id == 0 {
            return Some(("Outside atlas".to_owned(), None));
        }
        if let Some(r) = atlas.regions.get(&id) {
            Some((format!("{} — {}", r.abbr, r.name), Some(r.color)))
        } else {
            Some((format!("Region ID {id}"), Some(atlas.fallback)))
        }
    }
}

/// Load and validate a new atlas before replacing the current one.
pub fn load_atlas(
    path: &Path,
    reference: &Volume,
    wireframe: bool,
    labels_path: Option<&Path>,
    lut_path: Option<&Path>,
    fallback: [u8; 3],
    width: usize,
    opacity: f32,
) -> Result<Atlas> {
    let vol = Volume::load(path).with_context(|| format!("Loading atlas {}", path.display()))?;
    reference
        .validate_grid(&vol)
        .with_context(|| format!("Atlas {} is not voxel-aligned", path.display()))?;
    let labels = if let Some(label_path) = labels_path {
        let solid = Volume::load(label_path)
            .with_context(|| format!("Loading solid atlas labels {}", label_path.display()))?;
        reference.validate_grid(&solid).with_context(|| {
            format!(
                "Atlas labels {} are not voxel-aligned",
                label_path.display()
            )
        })?;
        Some(solid)
    } else {
        None
    };
    let regions = if let Some(csv_path) = lut_path {
        read_lut(csv_path)?
    } else {
        HashMap::new()
    };
    Ok(Atlas {
        vol,
        wireframe: wireframe,
        labels,
        fallback: fallback,
        width: width,
        opacity: opacity,
        regions,
    })
}

/// Read the UNRAVEL CCFv3-2020 CSV, including a possible UTF-8 BOM.
fn read_lut(path: &Path) -> Result<HashMap<i32, RegionInfo>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("Reading LUT {}", path.display()))?;
    let headers = reader
        .headers()?
        .iter()
        .map(|s| s.trim_start_matches('\u{feff}').trim().to_owned())
        .collect::<Vec<_>>();
    let col = |name: &str| -> Result<usize> {
        headers
            .iter()
            .position(|s| s == name)
            .with_context(|| format!("LUT {} lacks column {name}", path.display()))
    };
    let id_col = col("Region_ID")?;
    let name_col = col("Region")?;
    let abbr_col = col("Abbr")?;
    let general_col = headers.iter().position(|s| s == "General_Region");
    let rgb_cols = [col("R")?, col("G")?, col("B")?];
    let mut lut = HashMap::new();
    for record in reader.records() {
        let r = record?;
        let id: i32 = r
            .get(id_col)
            .context("LUT ID missing")?
            .trim()
            .parse()
            .context("Invalid Region_ID")?;
        let mut color = [0u8; 3];
        for (channel, i) in rgb_cols.iter().enumerate() {
            color[channel] = r
                .get(*i)
                .context("LUT RGB channel missing")?
                .trim()
                .parse()
                .with_context(|| format!("Invalid RGB value for region {id}"))?;
        }
        lut.insert(
            id,
            RegionInfo {
                name: r.get(name_col).unwrap_or("").to_owned(),
                abbr: r.get(abbr_col).unwrap_or("").to_owned(),
                color,
                general_region: general_col
                    .and_then(|i| r.get(i))
                    .unwrap_or("")
                    .trim()
                    .to_owned(),
            },
        );
    }
    Ok(lut)
}

#[inline]
pub fn label_id(v: f32) -> i32 {
    if v.is_finite() && v > 0.0 && v < i32::MAX as f32 {
        v.round() as i32
    } else {
        0
    }
}

impl Atlas {
    pub fn color(&self, id: i32) -> [u8; 3] {
        self.regions
            .get(&id)
            .map(|r| r.color)
            .unwrap_or(self.fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_unravel_lut_with_bom_and_quoted_region() {
        let mut p = std::env::temp_dir();
        p.push(format!("niv-lut-test-{}.csv", std::process::id()));
        std::fs::write(&p, "\u{feff}Region_ID,ID_Path,Region,Abbr,General_Region,R,G,B\n51,/997/,\"Nucleus, accumbens\",ACB,Striatum,128,205,248\n").unwrap();
        let got = read_lut(&p).unwrap();
        assert_eq!(got[&51].name, "Nucleus, accumbens");
        assert_eq!(got[&51].color, [128, 205, 248]);
        assert_eq!(got[&51].general_region, "Striatum");
        std::fs::remove_file(&p).unwrap();
    }
    #[test]
    fn atlas_region_inspection_uses_solid_labels() {
        use ndarray::Array3;
        let mk = |value: f32| Volume {
            data: Array3::from_elem((2, 2, 1), value),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let mut regions = HashMap::new();
        regions.insert(
            51,
            RegionInfo {
                color: [128, 205, 248],
                name: "Nucleus accumbens".into(),
                abbr: "ACB".into(),
                general_region: "Striatum__Ventral".into(),
            },
        );
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: mk(1.),
                wireframe: true,
                labels: Some(mk(51.)),
                fallback: [255; 3],
                width: 1,
                opacity: 1.0,
                regions,
            }),
        };
        assert_eq!(
            layers.region_at(Axis::Z, 0, 1, 1).as_deref(),
            Some("ACB — Nucleus accumbens (ID 51)")
        );
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;
    #[test]
    fn palettes_are_deterministic_and_have_expected_endpoints() {
        assert_eq!(Palette::Gray.rgb(0., 0), [0, 0, 0]);
        assert_eq!(Palette::Gray.rgb(1., 0), [255, 255, 255]);
        assert_eq!(Palette::Warm.rgb(0., 0), [255, 0, 0]);
        assert_eq!(Palette::Warm.rgb(1., 0), [255, 255, 0]);
        assert_eq!(Palette::Cool.rgb(0., 0), [0, 0, 255]);
        assert_eq!(Palette::Cool.rgb(1., 0), [0, 255, 255]);
        assert_eq!(Palette::Random.rgb(1., 1), Palette::Random.rgb(1., 1));
        assert_ne!(
            Palette::Random.voxel_color(1., 0., 100., 1),
            Palette::Random.voxel_color(2., 0., 100., 1)
        );
        assert_eq!(
            Palette::Random.voxel_color(7., 0., 100., 1),
            Palette::Random.voxel_color(7., 3., 12., 4)
        );
        assert_eq!(Palette::Warm.next(), Palette::Cool);
        assert_eq!(short_name(Path::new("/tmp/image.nii.gz")), "image");
    }
}

#[cfg(test)]
mod default_palette_tests {
    use super::*;
    #[test]
    fn recognizes_cluster_and_ci_filenames_only() {
        for p in [
            "cluster_index.nii.gz",
            "rev_CLUSTER.nii",
            "ci.nii.gz",
            "map_CI-LH.nii.gz",
        ] {
            assert_eq!(default_palette(Path::new(p)), Palette::Random);
        }
        for p in [
            "cingulate.nii.gz",
            "precision.nii.gz",
            "cluster_results/template.nii.gz",
        ] {
            assert_eq!(default_palette(Path::new(p)), Palette::Gray);
        }
    }
}
