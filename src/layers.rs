//! Voxel-aligned intensity overlays and colored atlas outlines.
//! On-the-fly outlines are rasterized at voxel FACE coordinates, not at centers.
use std::{collections::HashMap, path::Path};

use anyhow::{bail, Context, Result};

use crate::{config::Config, volume::{Axis, Volume}};

pub struct IntensityOverlay {
    pub name: String,
    pub vol: Volume,
    pub window: (f32, f32),
    pub color: [u8; 3],
    pub opacity: f32,
}

#[derive(Clone)]
pub struct RegionInfo {
    pub color: [u8; 3],
    pub name: String,
    pub abbr: String,
}

pub struct Atlas {
    pub vol: Volume,
    pub wireframe: bool,
    pub labels: Option<Volume>,
    pub fallback: [u8; 3],
    pub width: usize,
    pub regions: HashMap<i32, RegionInfo>,
}

pub struct Layers {
    pub overlays: Vec<IntensityOverlay>,
    pub atlas: Option<Atlas>,
}

impl Layers {
    pub fn load(config: &Config, reference: &Volume) -> Result<Self> {
        let mut overlays = Vec::new();
        for spec in &config.overlays {
            let vol = Volume::load(&spec.path)
                .with_context(|| format!("Loading intensity overlay {}", spec.path.display()))?;
            reference.validate_grid(&vol)
                .with_context(|| format!("Intensity overlay {} is not voxel-aligned", spec.path.display()))?;
            let window = spec.window.unwrap_or_else(|| vol.auto_window());
            let name = spec.path.file_name().map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| spec.path.display().to_string());
            overlays.push(IntensityOverlay { vol, window, color: spec.color, opacity: spec.opacity, name });
        }
        let atlas = if let Some(path) = &config.atlas_path {
            let vol = Volume::load(path).with_context(|| format!("Loading atlas {}", path.display()))?;
            reference.validate_grid(&vol)
                .with_context(|| format!("Atlas {} is not voxel-aligned", path.display()))?;
            let labels = if let Some(label_path) = &config.atlas_labels_path {
                let solid = Volume::load(label_path)
                    .with_context(|| format!("Loading solid atlas labels {}", label_path.display()))?;
                reference.validate_grid(&solid)
                    .with_context(|| format!("Atlas labels {} are not voxel-aligned", label_path.display()))?;
                Some(solid)
            } else { None };
            let regions = if let Some(csv_path) = &config.lut_path {
                read_lut(csv_path)?
            } else { HashMap::new() };
            Some(Atlas { vol, wireframe: config.atlas_wireframe, labels,
                fallback: config.atlas_color, width: config.atlas_width, regions })
        } else {
            if config.lut_path.is_some() { bail!("--lut requires --atlas or --atlas-wireframe"); }
            None
        };
        Ok(Self { overlays, atlas })
    }

    /// Full region text at a voxel, or None for unlabeled voxels. For a
    /// precomputed wireframe a separate solid atlas is required for reliable
    /// region lookup away from outline voxels.
    pub fn region_at(&self, axis: Axis, slice: usize, h: usize, v: usize) -> Option<String> {
        let atlas = self.atlas.as_ref()?;
        let labels = if atlas.wireframe { atlas.labels.as_ref()? } else { &atlas.vol };
        let id = label_id(labels.sample(axis, slice, h, v));
        if id == 0 { return Some("outside atlas".to_string()); }
        if let Some(reg) = atlas.regions.get(&id) {
            Some(format!("{} — {} (ID {})", reg.abbr, reg.name, id))
        } else { Some(format!("region ID {} (not in LUT)", id)) }
    }

}

/// Read the UNRAVEL CCFv3-2020 CSV, including a possible UTF-8 BOM.
fn read_lut(path: &Path) -> Result<HashMap<i32, RegionInfo>> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_path(path)
        .with_context(|| format!("Reading LUT {}", path.display()))?;
    let headers = reader.headers()?.iter().map(|s| s.trim_start_matches('\u{feff}').trim().to_owned())
        .collect::<Vec<_>>();
    let col = |name: &str| -> Result<usize> {
        headers.iter().position(|s| s == name)
            .with_context(|| format!("LUT {} lacks column {name}", path.display()))
    };
    let id_col = col("Region_ID")?;
    let name_col = col("Region")?;
    let abbr_col = col("Abbr")?;
    let rgb_cols = [col("R")?, col("G")?, col("B")?];
    let mut lut = HashMap::new();
    for record in reader.records() {
        let r = record?;
        let id: i32 = r.get(id_col).context("LUT ID missing")?.trim().parse().context("Invalid Region_ID")?;
        let mut color = [0u8; 3];
        for (channel, i) in rgb_cols.iter().enumerate() {
            color[channel] = r.get(*i).context("LUT RGB channel missing")?.trim().parse()
                .with_context(|| format!("Invalid RGB value for region {id}"))?;
        }
        lut.insert(id, RegionInfo {
            name: r.get(name_col).unwrap_or("").to_owned(),
            abbr: r.get(abbr_col).unwrap_or("").to_owned(),
            color,
        });
    }
    Ok(lut)
}

#[inline]
pub fn label_id(v: f32) -> i32 {
    if v.is_finite() && v > 0.0 && v < i32::MAX as f32 { v.round() as i32 } else { 0 }
}

impl Atlas {
    pub fn color(&self, id: i32) -> [u8; 3] {
        self.regions.get(&id).map(|r| r.color).unwrap_or(self.fallback)
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
        assert_eq!(got[&51].color, [128,205,248]);
        std::fs::remove_file(&p).unwrap();
    }
    #[test]
    fn atlas_region_inspection_uses_solid_labels() {
        use ndarray::Array3;
        let mk=|value:f32| Volume {data:Array3::from_elem((2,2,1),value),
            spacing:[1.;3],sform:None};
        let mut regions=HashMap::new();
        regions.insert(51, RegionInfo {color:[128,205,248],
            name:"Nucleus accumbens".into(),abbr:"ACB".into()});
        let layers=Layers {overlays:vec![],atlas:Some(Atlas {vol:mk(1.),
            wireframe:true, labels:Some(mk(51.)),fallback:[255;3],width:1,regions})};
        assert_eq!(layers.region_at(Axis::Z,0,1,1).as_deref(),
            Some("ACB — Nucleus accumbens (ID 51)"));
    }
}
