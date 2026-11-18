//! Paged montage rendered as a single Kitty graphics placement.
//! Each thumbnail uses the existing layered compositor and atlas boundary code.
//! Pages follow the selected slice; the entire volume is never rasterized at once.
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
    let scale = if app.pixel_budget > 0 && native_pixels > app.pixel_budget {
        (app.pixel_budget as f64 / native_pixels as f64).sqrt()
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
    // Leave at least one raster pixel per column even in very narrow terminals.
    let max_gap = if columns > 1 {
        width.saturating_sub(columns) / (columns - 1)
    } else {
        32
    };
    let gap = app.montage.gap.min(32).min(max_gap);
    let raw_tile_width = width
        .saturating_sub(gap.saturating_mul(columns.saturating_sub(1)))
        .checked_div(columns)
        .unwrap_or(1)
        .max(1);
    let tile_height = ((raw_tile_width as f64 / aspect).round() as usize).clamp(1, height);
    let tile_width = ((tile_height as f64 * aspect).round() as usize).clamp(1, raw_tile_width);
    let rows = ((height + gap) / (tile_height + gap)).clamp(1, 16);
    let range = app.montage.stop.saturating_sub(app.montage.start);
    let interval = app.montage.interval.max(1);
    let total_slices = range / interval + 1;
    let capacity = columns.saturating_mul(rows).max(1);
    let pages = total_slices.div_ceil(capacity);
    let selected = ((app.index.saturating_sub(app.montage.start) + interval / 2) / interval)
        .min(total_slices - 1);
    let page = selected / capacity;
    let first = page * capacity;
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
            slice: app.montage.start + (first + k) * interval,
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

fn draw_selection(
    frame: &mut [u8],
    width: usize,
    _height: usize,
    channels: usize,
    tile: &Tile,
    tw: usize,
    th: usize,
) {
    if tw < 5 || th < 5 {
        return;
    }
    let color = [255, 204, 75];
    for d in 0..2.min(tw / 2).min(th / 2) {
        for x in tile.x + d..tile.x + tw - d {
            put_pixel(frame, width, channels, x, tile.y + d, color);
            put_pixel(frame, width, channels, x, tile.y + th - 1 - d, color);
        }
        for y in tile.y + d..tile.y + th - d {
            put_pixel(frame, width, channels, tile.x + d, y, color);
            put_pixel(frame, width, channels, tile.x + tw - 1 - d, y, color);
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
    // Do not draw one crosshair or hover highlight on every thumbnail.
    tile_app.inspect_mode = false;
    tile_app.highlighted_region = None;
    for tile in &plan.tiles {
        tile_app.index = tile.slice;
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
        draw_label(
            &mut frame,
            plan.geom.width,
            plan.geom.height,
            channels,
            tile,
            plan.tile_width,
            plan.tile_height,
        );
        if tile.slice == app.index {
            draw_selection(
                &mut frame,
                plan.geom.width,
                plan.geom.height,
                channels,
                tile,
                plan.tile_width,
                plan.tile_height,
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
        app.pixel_budget = 100_000;
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
    fn page_changes_when_selection_leaves_current_page() {
        let v = vol();
        let mut app = App::new(Axis::Z, 0, 1, 90, (0., 100.), "test".into());
        app.reset_montage(&v);
        app.montage.enabled = true;
        app.montage.columns = 10;
        app.montage.interval = 1;
        let before = plan(&v, &app, 80, 20, 4);
        app.index = 29;
        let after = plan(&v, &app, 80, 20, 4);
        assert!(after.page >= before.page);
        assert!(after.tiles.iter().any(|t| t.slice == 29));
    }
}
