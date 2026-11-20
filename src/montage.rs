//! Paged montage rendered as a single Kitty graphics placement.
//! Each thumbnail uses the existing layered compositor and atlas boundary code.
//! Pages follow a page anchor independent of mouse inspection; the entire volume is never rasterized at once.
use crate::{
    app::App,
    layers::Layers,
    render::{self, Geometry, SamplingMap},
    volume::Volume,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    pub slice: usize,
    pub x: usize,
    pub y: usize,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub geom: Geometry,
    pub tile_width: usize,
    pub tile_height: usize,
    pub gap: usize,
    pub columns: usize,
    pub rows: usize,
    pub total_slices: usize,
    pub page: usize,
    pub pages: usize,
    pub tiles: Vec<Tile>,
}

/// Fit aspect-correct thumbnails into one full-viewport raster, keeping the
/// total pixel budget rather than allocating a budget for every tile.
pub fn plan(vol: &Volume, app: &App, term_cols: u16, term_rows: u16, reserved_rows: usize) -> Plan {
    let (cw, ch) = render::cell_pixels(term_cols, term_rows);
    let columns_cells = term_cols.saturating_sub(1).max(1) as usize;
    let rows_cells = (term_rows as usize).saturating_sub(reserved_rows).max(1);
    let native_width = columns_cells.saturating_mul(cw).max(1);
    let native_height = rows_cells.saturating_mul(ch).max(1);
    let native_pixels = native_width.saturating_mul(native_height);
    let scale = if app.montage_pixel_budget > 0 && native_pixels > app.montage_pixel_budget {
        (app.montage_pixel_budget as f64 / native_pixels as f64).sqrt()
    } else {
        1.0
    };
    let width = (native_width as f64 * scale).round().max(1.0) as usize;
    let height = (native_height as f64 * scale).round().max(1.0) as usize;
    let geom = Geometry {
        cols: columns_cells as u16,
        rows: rows_cells as u16,
        width,
        height,
    };

    let (_, _, mut aspect) = vol.plane(app.axis);
    if app.rotation % 180 != 0 {
        aspect = 1.0 / aspect;
    }
    let aspect = aspect.clamp(0.05, 20.0);
    let columns = app.montage.columns.clamp(1, 12).min(width);
    let rows = app.montage.rows.clamp(1, 12).min(height);
    let max_gap_x = if columns > 1 {
        (width - columns) / (columns - 1)
    } else {
        32
    };
    let max_gap_y = if rows > 1 {
        (height - rows) / (rows - 1)
    } else {
        32
    };
    let gap = app.montage.gap.min(32).min(max_gap_x).min(max_gap_y);
    let max_w = (width - gap * (columns - 1)) / columns;
    let max_h = (height - gap * (rows - 1)) / rows;
    let tile_height = ((max_w as f64 / aspect).round() as usize).clamp(1, max_h);
    let tile_width = ((tile_height as f64 * aspect).round() as usize).clamp(1, max_w);
    let total_slices = app.montage.count();
    let capacity = columns * rows;
    let pages = total_slices.div_ceil(columns).saturating_sub(rows) + 1;
    let page = (app.montage.position(app.index) / columns).min(pages - 1);
    let first = page * columns;
    let count = capacity.min(total_slices - first);
    // Center the visible set of rows/columns within the raster.
    let used_rows = count.div_ceil(columns);
    let used_columns = columns.min(count);
    let used_height = used_rows * tile_height + used_rows.saturating_sub(1) * gap;
    let used_width = used_columns * tile_width + used_columns.saturating_sub(1) * gap;
    let left = width.saturating_sub(used_width) / 2;
    let top = height.saturating_sub(used_height) / 2;
    let tiles = (0..count)
        .map(|k| Tile {
            slice: app.montage.slice_at(first + k),
            x: left + (k % columns) * (tile_width + gap),
            y: top + (k / columns) * (tile_height + gap),
        })
        .collect();
    Plan {
        geom,
        tile_width,
        tile_height,
        gap,
        columns,
        rows,
        total_slices,
        page,
        pages,
        tiles,
    }
}

/// Physical equivalent of the decorative tile gap, based on the in-plane
/// field of view and raster scale. This is not the distance between slices.
pub fn gap_micrometres(vol: &Volume, app: &App, plan: &Plan) -> Option<f64> {
    let (mut a, mut b) = app.axis.plane_axes();
    if app.rotation % 180 != 0 {
        std::mem::swap(&mut a, &mut b);
    }
    let dims = vol.dims();
    let sx = dims[a] as f64 * vol.spacing[a] as f64 / plan.tile_width as f64;
    let sy = dims[b] as f64 * vol.spacing[b] as f64 / plan.tile_height as f64;
    Some(plan.gap as f64 * (sx + sy) * 0.5 * vol.micrometre_scale()?)
}

fn put_pixel(frame: &mut [u8], width: usize, channels: usize, x: usize, y: usize, color: [u8; 3]) {
    let off = (y * width + x) * channels;
    frame[off..off + 3].copy_from_slice(&color);
    if channels == 4 {
        frame[off + 3] = 255;
    }
}

/// Small numerals identify each actual slice without reserving terminal rows.
const DIGITS: [[u8; 7]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b001, 0b111, 0b100, 0b100, 0b111],
    [0b111, 0b001, 0b001, 0b111, 0b001, 0b001, 0b111],
    [0b101, 0b101, 0b101, 0b111, 0b001, 0b001, 0b001],
    [0b111, 0b100, 0b100, 0b111, 0b001, 0b001, 0b111],
    [0b111, 0b100, 0b100, 0b111, 0b101, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b010, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b101, 0b111, 0b101, 0b101, 0b111],
    [0b111, 0b101, 0b101, 0b111, 0b001, 0b001, 0b111],
];

fn draw_label(
    frame: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    tile: &Tile,
    tw: usize,
    th: usize,
) {
    if tw < 18 || th < 14 {
        return;
    }
    let text = tile.slice.to_string();
    let label_width = text.len() * 4 + 5;
    if label_width > tw {
        return;
    }
    // Opaque dark backing makes labels readable over bright/colored images.
    for y in tile.y + 2..(tile.y + 13).min(height) {
        for x in tile.x + 2..(tile.x + label_width).min(width) {
            put_pixel(frame, width, channels, x, y, [12, 12, 16]);
        }
    }
    for (i, c) in text.bytes().enumerate() {
        let glyph = DIGITS[(c - b'0') as usize];
        for (gy, row) in glyph.iter().enumerate() {
            for gx in 0..3 {
                if (*row & (1u8 << (2 - gx))) != 0 {
                    put_pixel(
                        frame,
                        width,
                        channels,
                        tile.x + 4 + i * 4 + gx,
                        tile.y + 4 + gy,
                        [240, 240, 240],
                    );
                }
            }
        }
    }
}

/// Label the topmost visible categorical image (or the selected categorical
/// image when visible). Labels are numeric IDs, not atlas abbreviations.
fn cluster_labels(
    frame: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    tile: &Tile,
    app: &App,
    vol: &Volume,
    layers: &Layers,
    map: &SamplingMap,
) {
    use crate::layers::Palette;
    use std::collections::BTreeMap;
    let categorical = |id: usize| -> Option<(&Volume, (f32, f32))> {
        if id == 0 {
            (app.base_visible && app.base_palette == Palette::Random && app.base_opacity > 0.)
                .then_some((vol, app.window))
        } else {
            let o = layers.overlays.get(id - 1)?;
            (app.show_overlays && o.visible && o.opacity > 0. && o.palette == Palette::Random)
                .then_some((&o.vol, o.window))
        }
    };
    let Some((source, window)) = categorical(app.selected_layer_id()).or_else(|| {
        (0..=layers.overlays.len())
            .rev()
            .find_map(|pos| categorical(app.layer_at(pos)))
    }) else {
        return;
    };
    let tw = map.width;
    let th = map.height;
    if tw < 14 || th < 12 {
        return;
    }
    let read = |(h, v): (usize, usize)| {
        let v = source.sample(app.axis, tile.slice, h, v);
        if v.is_finite() && v > window.0 && v != 0. && (v - v.round()).abs() < 0.001 {
            Some(v.round() as i32)
        } else {
            None
        }
    };
    // Track the centroid, then find an actual pixel of the cluster closest to
    // it, so labels are anchored inside the cluster rather than in empty space.
    let mut ids: BTreeMap<i32, (usize, usize, usize)> = BTreeMap::new();
    for (i, &hv) in map.coords.iter().enumerate() {
        if let Some(id) = read(hv) {
            let entry = ids.entry(id).or_default();
            entry.0 += i % tw;
            entry.1 += i / tw;
            entry.2 += 1;
        }
    }
    let mut best: BTreeMap<i32, (f64, usize, usize)> = BTreeMap::new();
    for (i, &hv) in map.coords.iter().enumerate() {
        if let Some(id) = read(hv) {
            let (sx, sy, n) = ids[&id];
            let x = i % tw;
            let y = i / tw;
            let dx = x as f64 - sx as f64 / n as f64;
            let dy = y as f64 - sy as f64 / n as f64;
            let dist = dx * dx + dy * dy;
            let entry = best.entry(id).or_insert((f64::INFINITY, x, y));
            if dist < entry.0 {
                *entry = (dist, x, y);
            }
        }
    }
    // Large clusters first; don't place unreadable overlapping text.
    let mut ordered: Vec<_> = best.into_iter().collect();
    ordered.sort_by_key(|(id, _)| std::cmp::Reverse(ids[id].2));
    let mut boxes: Vec<(usize, usize, usize, usize)> = Vec::new();
    if app.montage.show_numbers {
        boxes.push((2, 2, tile.slice.to_string().len() * 4 + 5, 13));
    }
    let scale = if tw >= 210 && th >= 100 { 2 } else { 1 };
    for (id, (_, cx, cy)) in ordered {
        let text = id.to_string();
        let w = text.len() * 4 * scale + 2;
        let h = 7 * scale + 2;
        if w > tw || h > th {
            continue;
        }
        let x = cx.saturating_sub(w / 2).min(tw - w);
        let y = cy.saturating_sub(h / 2).min(th - h);
        if boxes
            .iter()
            .any(|&(a, b, c, d)| x < c && x + w > a && y < d && y + h > b)
        {
            continue;
        }
        boxes.push((x, y, x + w, y + h));
        let color = Palette::Random.voxel_color(id as f32, window.0, window.1, 0);
        // Dark small backing and foreground matching the categorical LUT.
        for yy in y..y + h {
            for xx in x..x + w {
                put_pixel(
                    frame,
                    width,
                    channels,
                    tile.x + xx,
                    tile.y + yy,
                    [12, 12, 16],
                );
            }
        }
        for (j, c) in text.bytes().enumerate() {
            let glyph = if c == b'-' {
                [0, 0, 0, 0b111, 0, 0, 0]
            } else {
                DIGITS[(c - b'0') as usize]
            };
            for (gy, row) in glyph.iter().enumerate() {
                for gx in 0..3 {
                    if *row & (1u8 << (2 - gx)) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = tile.x + x + 1 + j * 4 * scale + gx * scale + sx;
                                let py = tile.y + y + 1 + gy * scale + sy;
                                if px < width && py < height {
                                    put_pixel(frame, width, channels, px, py, color);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Every tile uses identical sampling coordinates; only its slice index
/// changes. The expensive voxel lookup is therefore computed once per page.
pub fn composite(
    vol: &Volume,
    app: &App,
    layers: &Layers,
    plan: &Plan,
    sampling: &SamplingMap,
) -> Vec<u8> {
    let channels = if app.transparent_background { 4 } else { 3 };
    let mut frame = vec![0u8; plan.geom.width * plan.geom.height * channels];
    let mut tile_app = app.clone();
    // Draw the optional delayed outline only on the probed thumbnail.
    tile_app.inspect_mode = false;
    for tile in &plan.tiles {
        tile_app.index = tile.slice;
        tile_app.highlighted_region = if app.montage_highlight_slice == Some(tile.slice) {
            app.highlighted_region
        } else {
            None
        };
        let pixels = render::composite_with_map(
            vol,
            &tile_app,
            layers,
            sampling,
            app.transparent_background,
        );
        for row in 0..plan.tile_height {
            let source = row * plan.tile_width * channels;
            let target = ((tile.y + row) * plan.geom.width + tile.x) * channels;
            frame[target..target + plan.tile_width * channels]
                .copy_from_slice(&pixels[source..source + plan.tile_width * channels]);
        }
        if app.montage.show_numbers {
            draw_label(
                &mut frame,
                plan.geom.width,
                plan.geom.height,
                channels,
                tile,
                plan.tile_width,
                plan.tile_height,
            );
        }
        if app.montage.show_clusters {
            cluster_labels(
                &mut frame,
                plan.geom.width,
                plan.geom.height,
                channels,
                tile,
                app,
                vol,
                layers,
                sampling,
            );
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{layers::Layers, volume::Axis};
    use ndarray::Array3;
    fn vol() -> Volume {
        Volume {
            data: Array3::from_shape_fn((12, 20, 30), |(x, y, z)| (x * 1000 + y * 30 + z) as f32),
            spacing: [1.; 3],
            spatial_units: 2,
            sform: None,
            datatype: 16,
        }
    }
    #[test]
    fn montage_pages_preserve_slice_order_spacing_and_budget() {
        let v = vol();
        let mut app = App::new(Axis::Z, 11, 1, 90, (0., 30000.), "test".into());
        app.reset_montage(&v);
        app.montage.enabled = true;
        app.montage.start = 2;
        app.montage.stop = 25;
        app.montage.interval = 3;
        app.montage.columns = 3;
        app.montage_pixel_budget = 100_000;
        let p = plan(&v, &app, 100, 40, 3);
        assert!(p.geom.width * p.geom.height <= 101_000);
        assert_eq!(p.total_slices, 8);
        assert!(p
            .tiles
            .iter()
            .all(|t| (t.slice - 2) % 3 == 0 && t.slice <= 25));
        let sample = SamplingMap::new(&v, &app, p.tile_width, p.tile_height);
        let rgb = composite(&v, &app, &Layers::default(), &p, &sample);
        assert_eq!(rgb.len(), p.geom.width * p.geom.height * 3);
    }
    #[test]
    fn hover_dilation_is_confined_to_one_tile() {
        let v = vol();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 30000.), "test".into());
        app.reset_montage(&v);
        app.montage.enabled = true;
        let labels = Volume {
            data: Array3::from_shape_fn((12, 20, 30), |(x, _, _)| if x < 6 { 1. } else { 2. }),
            spacing: [1.; 3],
            spatial_units: 2,
            sform: None,
            datatype: 16,
        };
        let layers = Layers {
            overlays: vec![],
            atlas: Some(crate::layers::Atlas {
                vol: labels,
                wireframe: false,
                labels: None,
                fallback: [255, 0, 0],
                width: 1,
                opacity: 1.,
                regions: Default::default(),
            }),
        };
        let p = plan(&v, &app, 100, 40, 3);
        let map = SamplingMap::new(&v, &app, p.tile_width, p.tile_height);
        let plain = composite(&v, &app, &layers, &p, &map);
        let tile = p.tiles[0];
        app.montage_highlight_slice = Some(tile.slice);
        app.highlighted_region = Some(1);
        let highlighted = composite(&v, &app, &layers, &p, &map);
        let mut changed = 0;
        for (i, (before, after)) in plain
            .chunks_exact(3)
            .zip(highlighted.chunks_exact(3))
            .enumerate()
        {
            if before != after {
                let x = i % p.geom.width;
                let y = i / p.geom.width;
                assert!(
                    x >= tile.x
                        && x < tile.x + p.tile_width
                        && y >= tile.y
                        && y < tile.y + p.tile_height
                );
                changed += 1;
            }
        }
        assert!(changed > 0);
    }
    #[test]
    fn physical_gap_uses_in_plane_raster_scale() {
        let mut v = vol();
        v.spacing = [0.03; 3];
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "test".into());
        app.reset_montage(&v);
        app.montage.enabled = true;
        let mut p = plan(&v, &app, 100, 40, 3);
        // At one raster pixel per voxel, eight pixels = 240 µm.
        p.tile_width = 12;
        p.tile_height = 20;
        p.gap = 8;
        assert!((gap_micrometres(&v, &app, &p).unwrap() - 240.).abs() < 1e-3);
        app.rotation = 90;
        p.tile_width = 40;
        p.tile_height = 24;
        assert!((gap_micrometres(&v, &app, &p).unwrap() - 120.).abs() < 1e-3);
    }
    #[test]
    fn default_grid_has_twelve_clean_tiles_and_includes_both_bounds() {
        let v = vol();
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 30000.), "test".into());
        app.reset_montage(&v);
        app.montage.enabled = true;
        app.index = app.montage.start;
        let p = plan(&v, &app, 100, 40, 3);
        assert_eq!((p.rows, p.columns), (3, 4));
        assert_eq!(p.tiles.len(), 12);
        assert_eq!(p.tiles.first().unwrap().slice, app.montage.start);
        assert_eq!(p.tiles.last().unwrap().slice, app.montage.stop);
        let sample = SamplingMap::new(&v, &app, p.tile_width, p.tile_height);
        let image = composite(&v, &app, &Layers::default(), &p, &sample);
        app.index = app.montage.stop;
        let other = plan(&v, &app, 100, 40, 3);
        assert_eq!(other.tiles, p.tiles);
        assert_eq!(
            composite(&v, &app, &Layers::default(), &other, &sample),
            image
        );
        app.montage.show_numbers = true;
        assert_ne!(
            composite(&v, &app, &Layers::default(), &other, &sample),
            image
        );
    }
    #[test]
    fn categorical_cluster_labels_use_the_source_palette() {
        let v = Volume {
            data: Array3::from_elem((20, 20, 12), 2.),
            spacing: [1.; 3],
            spatial_units: 2,
            sform: None,
            datatype: 16,
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 4.), "clusters".into());
        app.reset_montage(&v);
        app.montage.enabled = true;
        app.base_palette = crate::layers::Palette::Random;
        app.index = app.montage.start;
        let plan = plan(&v, &app, 100, 40, 3);
        let map = SamplingMap::new(&v, &app, plan.tile_width, plan.tile_height);
        let plain = composite(&v, &app, &Layers::default(), &plan, &map);
        app.montage.show_clusters = true;
        let annotated = composite(&v, &app, &Layers::default(), &plan, &map);
        assert_ne!(plain, annotated);
        let color = app.base_palette.voxel_color(2., 0., 4., 0);
        assert!(annotated
            .chunks_exact(3)
            .all(|p| p == color.as_slice() || p == &[0u8, 0, 0][..] || p == &[12u8, 12, 16][..]));
    }
    #[test]
    fn page_changes_when_selection_leaves_current_page() {
        let v = vol();
        let mut app = App::new(Axis::Z, 0, 1, 90, (0., 100.), "test".into());
        app.reset_montage(&v);
        app.montage.enabled = true;
        app.montage.columns = 10;
        app.montage.rows = 1;
        app.montage.start = 0;
        app.montage.stop = 29;
        app.montage.interval = 1;
        let before = plan(&v, &app, 80, 20, 4);
        app.index = 29;
        let after = plan(&v, &app, 80, 20, 4);
        assert!(after.page >= before.page);
        assert!(after.tiles.iter().any(|t| t.slice == 29));
    }
}
