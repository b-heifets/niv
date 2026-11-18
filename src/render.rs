use crossterm::terminal;

use crate::{app::App, volume::Volume};

#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub width: usize,
    pub height: usize,
}

/// Size the image to terminal cell boundaries so geometry changes do not
/// unexpectedly cover the footer. Physical aspect ratio follows voxel spacing.
pub fn geometry(
    vol: &Volume,
    app: &App,
    term_cols: u16,
    term_rows: u16,
    reserved_rows: usize,
) -> Geometry {
    let (_, _, mut aspect) = vol.plane(app.axis);
    if app.rotation % 180 != 0 {
        aspect = 1.0 / aspect;
    }
    let usable_cols = term_cols.saturating_sub(1).max(1) as usize;
    let usable_rows = (term_rows as usize).saturating_sub(reserved_rows).max(1);
    let (cell_w, cell_h) = cell_pixels(term_cols, term_rows);
    let viewport_w = usable_cols * cell_w;
    let viewport_h = usable_rows * cell_h;

    let (wanted_w, wanted_h) = if viewport_w as f64 / viewport_h as f64 > aspect {
        (
            (viewport_h as f64 * aspect).round().max(1.) as usize,
            viewport_h,
        )
    } else {
        (
            viewport_w,
            (viewport_w as f64 / aspect).round().max(1.) as usize,
        )
    };
    let cols = ((wanted_w / cell_w).max(1)).min(usable_cols) as u16;
    let rows = ((wanted_h / cell_h).max(1)).min(usable_rows) as u16;
    let view_w = cols as usize * cell_w;
    let view_h = rows as usize * cell_h;
    Geometry {
        cols,
        rows,
        width: if app.pixel_budget > 0 && view_w.saturating_mul(view_h) > app.pixel_budget {
            ((view_w as f64 * (app.pixel_budget as f64 / (view_w * view_h) as f64).sqrt()).round()
                as usize)
                .max(1)
        } else {
            view_w.max(1)
        },
        height: if app.pixel_budget > 0 && view_w.saturating_mul(view_h) > app.pixel_budget {
            ((view_h as f64 * (app.pixel_budget as f64 / (view_w * view_h) as f64).sqrt()).round()
                as usize)
                .max(1)
        } else {
            view_h.max(1)
        },
    }
}

pub(crate) fn cell_pixels(cols: u16, rows: u16) -> (usize, usize) {
    if let Ok(ws) = terminal::window_size() {
        if ws.width > 0 && ws.height > 0 && cols > 0 && rows > 0 {
            return (
                (ws.width as usize / cols as usize).max(1),
                (ws.height as usize / rows as usize).max(1),
            );
        }
    }
    (8, 16)
}

/// Nearest neighbor. Rotations are *screen-space*, clockwise in 90-degree
/// increments, not affine-based reorientation. Screen-space flips apply AFTER rotation.
#[cfg(test)]
pub fn grayscale(vol: &Volume, app: &App, width: usize, height: usize) -> Vec<u8> {
    let (source_w, source_h, _) = vol.plane(app.axis);
    let mut raster = vec![0u8; width * height * 3];
    let (lo, hi) = app.window;
    let gain = 255.0 / (hi - lo).max(f32::MIN_POSITIVE);
    let swapped = app.rotation % 180 != 0;
    let oriented_w = if swapped { source_h } else { source_w };
    let oriented_h = if swapped { source_w } else { source_h };
    let horizontal: Vec<usize> = (0..width)
        .map(|x| {
            let index = (x * oriented_w / width).min(oriented_w - 1);
            if app.flip_h {
                oriented_w - 1 - index
            } else {
                index
            }
        })
        .collect();
    let vertical: Vec<usize> = (0..height)
        .map(|y| {
            let index = (y * oriented_h / height).min(oriented_h - 1);
            if app.flip_v {
                oriented_h - 1 - index
            } else {
                index
            }
        })
        .collect();

    for (y, row) in raster.chunks_exact_mut(width * 3).enumerate() {
        let out_v = vertical[y];
        for (x, px) in row.chunks_exact_mut(3).enumerate() {
            let out_h = horizontal[x];
            let (h, v) = match app.rotation {
                0 => (out_h, source_h - 1 - out_v),
                90 => (out_v, out_h),
                180 => (source_w - 1 - out_h, out_v),
                270 => (source_w - 1 - out_v, source_h - 1 - out_h),
                _ => unreachable!("rotation must be 0, 90, 180, or 270"),
            };
            let intensity = vol.sample(app.axis, app.index, h, v);
            if !intensity.is_finite() {
                px.copy_from_slice(&[0; 3]);
                continue;
            }
            let level = ((intensity - lo) * gain / 255.).clamp(0., 1.);
            px.copy_from_slice(&app.base_palette.rgb(level, 0));
        }
    }
    raster
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::Axis;
    use ndarray::Array3;

    #[test]
    fn cached_sampling_matches_uncached_after_slice_change() {
        use crate::layers::Layers;
        let vol = Volume {
            data: Array3::from_shape_fn((4, 5, 6), |(x, y, z)| (x * 20 + y * 4 + z) as f32),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let mut app = App::new(Axis::X, 1, 1, 90, (0., 120.), "test".into());
        let map = SamplingMap::new(&vol, &app, 40, 50);
        assert!(map.matches(&vol, &app, 40, 50));
        assert_eq!(
            composite_with_map(&vol, &app, &Layers::default(), &map, false),
            composite_format(&vol, &app, &Layers::default(), 40, 50, false)
        );
        app.index = 2;
        assert!(map.matches(&vol, &app, 40, 50));
        assert_eq!(
            composite_with_map(&vol, &app, &Layers::default(), &map, false),
            composite_format(&vol, &app, &Layers::default(), 40, 50, false)
        );
        app.rotation = 180;
        assert!(!map.matches(&vol, &app, 40, 50));
    }
    #[test]
    fn flips_and_rotations() {
        // Data contains displayed rows [40,50], [20,30], [0,10] (top to bottom).
        // We compare grayscale red channel at each output pixel.
        let vol = Volume {
            data: Array3::from_shape_fn((2, 3, 1), |(x, y, _)| (10 * x + 20 * y) as f32),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 255.), "test".into());
        let red = |rgb: Vec<u8>| rgb.chunks_exact(3).map(|p| p[0]).collect::<Vec<_>>();
        assert_eq!(red(grayscale(&vol, &app, 2, 3)), [40, 50, 20, 30, 0, 10]);
        app.rotation = 90;
        assert_eq!(red(grayscale(&vol, &app, 3, 2)), [0, 20, 40, 10, 30, 50]);
        app.rotation = 180;
        assert_eq!(red(grayscale(&vol, &app, 2, 3)), [10, 0, 30, 20, 50, 40]);
        app.rotation = 270;
        assert_eq!(red(grayscale(&vol, &app, 3, 2)), [50, 30, 10, 40, 20, 0]);
        app.rotation = 0;
        app.flip_h = true;
        assert_eq!(red(grayscale(&vol, &app, 2, 3)), [50, 40, 30, 20, 10, 0]);
        app.flip_v = true;
        assert_eq!(red(grayscale(&vol, &app, 2, 3)), [10, 0, 30, 20, 50, 40]);
    }
}

/// Composite layers in RAM. RGB: blend against black, then send Kitty f=24.
/// RGBA: preserve accumulated alpha for true terminal-background transparency.
/// The display-minimum threshold excludes voxels in BOTH formats.
#[inline]
fn blend_pixel(dst: &mut [u8], color: [u8; 3], alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    if dst.len() == 3 {
        // The common case (fully opaque grayscale or atlas) needs no float math.
        if alpha >= 1.0 {
            dst.copy_from_slice(&color);
            return;
        }
        // Opaque black canvas: source-over blending with no alpha channel.
        for channel in 0..3 {
            dst[channel] = (color[channel] as f32 * alpha + dst[channel] as f32 * (1.0 - alpha))
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    } else {
        // Straight (unassociated) alpha: no premultiplication in Kitty payload.
        let previous = dst[3] as f32 / 255.0;
        let combined = alpha + previous * (1.0 - alpha);
        if combined <= 0.0 {
            return;
        }
        for channel in 0..3 {
            dst[channel] = ((color[channel] as f32 * alpha
                + dst[channel] as f32 * previous * (1.0 - alpha))
                / combined)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        dst[3] = (combined * 255.0).round() as u8;
    }
}

/// Bottom image, overlays in load order, and atlas last. All layers use the
/// same registration grid; no image is resized independently of the others.
/// Precomputed screen-to-voxel mapping, kept across slice changes.
/// The expensive per-pixel coordinate transform is only rebuilt if geometry,
/// axis, rotation or flips change.
pub struct SamplingMap {
    pub width: usize,
    pub height: usize,
    axis: crate::volume::Axis,
    rotation: u16,
    flip_h: bool,
    flip_v: bool,
    sw: usize,
    sh: usize,
    coords: Vec<(usize, usize)>,
}
impl SamplingMap {
    pub fn matches(&self, vol: &Volume, app: &App, width: usize, height: usize) -> bool {
        let (sw, sh, _) = vol.plane(app.axis);
        self.width == width
            && self.height == height
            && self.axis == app.axis
            && self.rotation == app.rotation
            && self.flip_h == app.flip_h
            && self.flip_v == app.flip_v
            && self.sw == sw
            && self.sh == sh
    }
    pub fn new(vol: &Volume, app: &App, width: usize, height: usize) -> Self {
        let (sw, sh, _) = vol.plane(app.axis);
        let swapped = app.rotation % 180 != 0;
        let ow = if swapped { sh } else { sw };
        let oh = if swapped { sw } else { sh };
        let horizontal: Vec<usize> = (0..width)
            .map(|x| {
                let h = (x * ow / width).min(ow - 1);
                if app.flip_h {
                    ow - 1 - h
                } else {
                    h
                }
            })
            .collect();
        let vertical: Vec<usize> = (0..height)
            .map(|y| {
                let v = (y * oh / height).min(oh - 1);
                if app.flip_v {
                    oh - 1 - v
                } else {
                    v
                }
            })
            .collect();
        let coords: Vec<(usize, usize)> = (0..height)
            .flat_map(|y| {
                let horizontal = &horizontal;
                let vertical = &vertical;
                (0..width).map(move |x| {
                    inverse_rotation(horizontal[x], vertical[y], sw, sh, app.rotation)
                })
            })
            .collect();
        Self {
            width,
            height,
            axis: app.axis,
            rotation: app.rotation,
            flip_h: app.flip_h,
            flip_v: app.flip_v,
            sw,
            sh,
            coords,
        }
    }
}

#[cfg(test)]
pub fn composite_format(
    vol: &Volume,
    app: &App,
    layers: &crate::layers::Layers,
    width: usize,
    height: usize,
    transparent_background: bool,
) -> Vec<u8> {
    let map = SamplingMap::new(vol, app, width, height);
    composite_with_map(vol, app, layers, &map, transparent_background)
}

pub fn composite_with_map(
    vol: &Volume,
    app: &App,
    layers: &crate::layers::Layers,
    map: &SamplingMap,
    transparent_background: bool,
) -> Vec<u8> {
    let width = map.width;
    let height = map.height;
    let sw = map.sw;
    let sh = map.sh;
    let coords = &map.coords;
    let channels = if transparent_background { 4 } else { 3 };
    let mut raster = vec![0u8; width * height * channels];
    // Composite images from bottom to top. IDs refer to loaded images;
    // stack positions can change without moving or reloading large volumes.
    for position in 0..=layers.overlays.len() {
        let id = app.layer_at(position);
        if id == 0 {
            if app.base_opacity <= 0.0 {
                continue;
            }
            for (px, &(h, v)) in raster.chunks_exact_mut(channels).zip(coords.iter()) {
                let value = vol.sample(app.axis, app.index, h, v);
                if !value.is_finite() || value <= app.window.0 {
                    continue;
                }
                let color = app
                    .base_palette
                    .voxel_color(value, app.window.0, app.window.1, 0);
                blend_pixel(px, color, app.base_opacity);
            }
        } else {
            if !app.show_overlays {
                continue;
            }
            let layer = &layers.overlays[id - 1];
            if !layer.visible || layer.opacity <= 0.0 {
                continue;
            }
            let (lo, hi) = layer.window;
            for (px, &(h, v)) in raster.chunks_exact_mut(channels).zip(coords.iter()) {
                let value = layer.vol.sample(app.axis, app.index, h, v);
                if !value.is_finite() || value <= lo {
                    continue;
                }
                let color = layer.palette.voxel_color(value, lo, hi, id);
                blend_pixel(px, color, layer.opacity);
            }
        }
    }
    if app.show_atlas {
        if let Some(atlas) = &layers.atlas {
            if atlas.opacity > 0.0 {
                if atlas.wireframe {
                    draw_wireframe(&mut raster, width, height, channels, app, atlas, coords);
                } else {
                    draw_atlas_edges(&mut raster, width, height, channels, app, atlas, sw, sh);
                }
            }
        }
    }
    if app.inspect_mode {
        if let Some((h, v)) = app.probe {
            if h < sw && v < sh {
                draw_inspector_mark(&mut raster, width, height, channels, app, sw, sh, h, v);
            }
        }
    }
    raster
}

/// Retained for tests and compatibility; the interactive path selects RGB/RGBA.
#[cfg(test)]
pub fn composite(
    vol: &Volume,
    app: &App,
    layers: &crate::layers::Layers,
    width: usize,
    height: usize,
) -> Vec<u8> {
    composite_format(vol, app, layers, width, height, true)
}

/// Inverse display mapping. The exact same mapping as the grayscale raster.
/// Coordinates here refer to raster pixels (not terminal cells).
pub fn voxel_at_raster(
    vol: &Volume,
    app: &App,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Option<(usize, usize)> {
    if x >= width || y >= height || width == 0 || height == 0 {
        return None;
    }
    let (sw, sh, _) = vol.plane(app.axis);
    let swapped = app.rotation % 180 != 0;
    let ow = if swapped { sh } else { sw };
    let oh = if swapped { sw } else { sh };
    let mut ox = (x * ow / width).min(ow - 1);
    let mut oy = (y * oh / height).min(oh - 1);
    if app.flip_h {
        ox = ow - 1 - ox;
    }
    if app.flip_v {
        oy = oh - 1 - oy;
    }
    Some(inverse_rotation(ox, oy, sw, sh, app.rotation))
}

#[inline]
fn inverse_rotation(x: usize, y: usize, sw: usize, sh: usize, rotate: u16) -> (usize, usize) {
    match rotate {
        0 => (x, sh - 1 - y),
        90 => (y, x),
        180 => (sw - 1 - x, y),
        270 => (sw - 1 - y, sh - 1 - x),
        _ => unreachable!(),
    }
}

/// Map integer source voxel FACE coordinates to output pixel edges.
/// The atlas is drawn AFTER all intensity overlays and therefore cannot be
/// occluded by any of them. Edges stay between the nearest-neighbor cells.
fn project(
    app: &App,
    h: usize,
    v: usize,
    sw: usize,
    sh: usize,
    width: usize,
    height: usize,
) -> (i32, i32) {
    let (mut x, mut y, ow, oh) = match app.rotation {
        0 => (h, sh - v, sw, sh),
        90 => (v, h, sh, sw),
        180 => (sw - h, v, sw, sh),
        270 => (sh - v, sw - h, sh, sw),
        _ => unreachable!(),
    };
    if app.flip_h {
        x = ow - x;
    }
    if app.flip_v {
        y = oh - y;
    }
    let px = ((x as f64 * width as f64 / ow as f64).round() as i32).clamp(0, width as i32 - 1);
    let py = ((y as f64 * height as f64 / oh as f64).round() as i32).clamp(0, height as i32 - 1);
    (px, py)
}

fn paint(
    rgba: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    x: i32,
    y: i32,
    color: [u8; 3],
    thick: usize,
    opacity: f32,
) {
    let neg = ((thick - 1) / 2) as i32;
    for yy in y - neg..y - neg + thick as i32 {
        for xx in x - neg..x - neg + thick as i32 {
            if xx >= 0 && yy >= 0 && (xx as usize) < width && (yy as usize) < height {
                let off = (yy as usize * width + xx as usize) * channels;
                blend_pixel(&mut rgba[off..off + channels], color, opacity);
            }
        }
    }
}

/// A temporary per-pixel coverage/color buffer for atlas strokes. Each pixel
/// is composited exactly once, so intersections, voxel corners and overlapping
/// boundary segments cannot become brighter through repeated alpha blending.
///
/// Packed bits: priority (31), coverage 0..127 (24..30), RGB (0..23).
/// Hovered-region strokes have higher priority than ordinary atlas edges.
struct StrokeCanvas {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    touched: Vec<usize>,
}

impl StrokeCanvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height],
            touched: Vec::new(),
        }
    }

    #[inline]
    fn plot(&mut self, x: i32, y: i32, color: [u8; 3], coverage: f32, selected: bool) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let alpha = (coverage.clamp(0.0, 1.0) * 127.0).round() as u32;
        if alpha == 0 {
            return;
        }
        let idx = y as usize * self.width + x as usize;
        let old = self.pixels[idx];
        let old_priority = old >> 31;
        let priority = if selected { 1u32 } else { 0u32 };
        let previous_coverage = (old >> 24) & 127;
        if old_priority > priority || (old_priority == priority && previous_coverage >= alpha) {
            return;
        }
        // When a hovered outline takes priority over an ordinary edge,
        // preserve at least the old coverage so that a soft hover fringe
        // cannot make an underlying solid atlas line *dimmer*.
        let alpha = if priority > old_priority {
            alpha.max(previous_coverage)
        } else {
            alpha
        };
        let encoded = (priority << 31)
            | (alpha << 24)
            | ((color[0] as u32) << 16)
            | ((color[1] as u32) << 8)
            | color[2] as u32;
        if old == 0 {
            self.touched.push(idx);
        }
        self.pixels[idx] = encoded;
    }

    #[inline]
    fn box_stamp(&mut self, x: i32, y: i32, color: [u8; 3], thick: usize, selected: bool) {
        let radius = ((thick - 1) / 2) as i32;
        for yy in y - radius..y - radius + thick as i32 {
            for xx in x - radius..x - radius + thick as i32 {
                self.plot(xx, yy, color, 1.0, selected);
            }
        }
    }

    /// Soft-edged circular brush for precomputed wireframe highlights.
    /// This alters only the stroke coverage, never the label or probe position.
    fn soft_stamp(&mut self, x: i32, y: i32, color: [u8; 3], thick: usize) {
        let radius = (thick as f32 * 0.5).max(1.5);
        let extent = (radius + 0.85).ceil() as i32;
        for yy in y - extent..=y + extent {
            for xx in x - extent..=x + extent {
                let dx = xx as f32 + 0.5 - x as f32;
                let dy = yy as f32 + 0.5 - y as f32;
                let dist = (dx * dx + dy * dy).sqrt();
                let coverage = (radius + 0.5 - dist).clamp(0.0, 1.0);
                self.plot(xx, yy, color, coverage, true);
            }
        }
    }

    fn blend_into(self, raster: &mut [u8], channels: usize, opacity: f32) {
        for idx in self.touched {
            let packed = self.pixels[idx];
            let coverage = ((packed >> 24) & 127) as f32 / 127.0;
            let color = [(packed >> 16) as u8, (packed >> 8) as u8, packed as u8];
            let offset = idx * channels;
            blend_pixel(
                &mut raster[offset..offset + channels],
                color,
                opacity * coverage,
            );
        }
    }
}

/// Subpixel projection of voxel FACE positions; the atlas's anatomical
/// boundaries stay exactly between voxels, including when antialiasing.
fn project_subpixel(
    app: &App,
    h: f32,
    v: f32,
    sw: usize,
    sh: usize,
    width: usize,
    height: usize,
) -> (f32, f32) {
    let sw = sw as f32;
    let sh = sh as f32;
    let (mut x, mut y, ow, oh) = match app.rotation {
        0 => (h, sh - v, sw, sh),
        90 => (v, h, sh, sw),
        180 => (sw - h, v, sw, sh),
        270 => (sh - v, sw - h, sh, sw),
        _ => unreachable!(),
    };
    if app.flip_h {
        x = ow - x;
    }
    if app.flip_v {
        y = oh - y;
    }
    (x * width as f32 / ow, y * height as f32 / oh)
}

fn stroke_face(
    strokes: &mut StrokeCanvas,
    app: &App,
    sw: usize,
    sh: usize,
    p: (usize, usize),
    q: (usize, usize),
    color: [u8; 3],
    thick: usize,
    selected: bool,
) {
    let width = strokes.width;
    let height = strokes.height;
    if !app.atlas_aa {
        let (x0, y0) = project(app, p.0, p.1, sw, sh, width, height);
        let (x1, y1) = project(app, q.0, q.1, sw, sh, width, height);
        if x0 == x1 {
            for y in y0.min(y1)..=y0.max(y1) {
                strokes.box_stamp(x0, y, color, thick, selected);
            }
        } else {
            for x in x0.min(x1)..=x0.max(x1) {
                strokes.box_stamp(x, y0, color, thick, selected);
            }
        }
        return;
    }

    let (x0, y0) = project_subpixel(app, p.0 as f32, p.1 as f32, sw, sh, width, height);
    let (x1, y1) = project_subpixel(app, q.0 as f32, q.1 as f32, sw, sh, width, height);
    let vertical = (x1 - x0).abs() < (y1 - y0).abs();
    // A centered 1.25px stroke (3px for hovered regions) with a smooth
    // ~0.8px edge. The centerline is unchanged; only edge coverage is filtered.
    let half_core = if selected {
        thick as f32 * 0.5 - 0.4
    } else {
        thick as f32 * 0.5 - 0.2
    };
    let feather = 0.8_f32;
    let (min_x, max_x, min_y, max_y) = if vertical {
        (
            x0 - half_core - feather,
            x0 + half_core + feather,
            y0.min(y1) - 0.5,
            y0.max(y1) + 0.5,
        )
    } else {
        (
            x0.min(x1) - 0.5,
            x0.max(x1) + 0.5,
            y0 - half_core - feather,
            y0 + half_core + feather,
        )
    };
    let xa = min_x.floor().max(0.0) as i32;
    let xb = max_x.ceil().min(width as f32) as i32;
    let ya = min_y.floor().max(0.0) as i32;
    let yb = max_y.ceil().min(height as f32) as i32;
    for y in ya..yb {
        let cy = y as f32 + 0.5;
        for x in xa..xb {
            let cx = x as f32 + 0.5;
            let perp_dist = if vertical {
                (cx - x0).abs()
            } else {
                (cy - y0).abs()
            };
            let cross_coverage = ((half_core + feather - perp_dist) / feather).clamp(0.0, 1.0);
            let along = if vertical {
                (cy - y0.min(y1)).min(y0.max(y1) - cy)
            } else {
                (cx - x0.min(x1)).min(x0.max(x1) - cx)
            };
            let end_coverage = (along + 0.5).clamp(0.0, 1.0);
            strokes.plot(x, y, color, cross_coverage * end_coverage, selected);
        }
    }
}

fn draw_inspector_mark(
    rgb: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    app: &App,
    sw: usize,
    sh: usize,
    h: usize,
    v: usize,
) {
    let (x, y) = project_subpixel(app, h as f32 + 0.5, v as f32 + 0.5, sw, sh, width, height);
    let x = x.round() as i32;
    let y = y.round() as i32;
    for d in -4..=4 {
        paint(rgb, width, height, channels, x + d, y, [0, 0, 0], 3, 1.0);
        paint(rgb, width, height, channels, x, y + d, [0, 0, 0], 3, 1.0);
    }
    for d in -3..=3 {
        paint(
            rgb,
            width,
            height,
            channels,
            x + d,
            y,
            [255, 255, 255],
            1,
            1.0,
        );
        paint(
            rgb,
            width,
            height,
            channels,
            x,
            y + d,
            [255, 255, 255],
            1,
            1.0,
        );
    }
}

fn draw_atlas_edges(
    rgb: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    app: &App,
    atlas: &crate::layers::Atlas,
    sw: usize,
    sh: usize,
) {
    use crate::layers::label_id;
    let mut strokes = StrokeCanvas::new(width, height);
    // Horizontal-axis neighbors: draw face at h, for each v.
    for v in 0..sh {
        for h in 0..=sw {
            let left = if h == 0 {
                0
            } else {
                label_id(atlas.vol.sample(app.axis, app.index, h - 1, v))
            };
            let right = if h == sw {
                0
            } else {
                label_id(atlas.vol.sample(app.axis, app.index, h, v))
            };
            if left != right && (left != 0 || right != 0) {
                let selected = app
                    .highlighted_region
                    .is_some_and(|id| id == left || id == right);
                let id = if selected {
                    app.highlighted_region.unwrap_or(right)
                } else if right != 0 {
                    right
                } else {
                    left
                };
                let thick = if selected {
                    atlas.width.max(3)
                } else {
                    atlas.width
                };
                stroke_face(
                    &mut strokes,
                    app,
                    sw,
                    sh,
                    (h, v),
                    (h, v + 1),
                    atlas.color(id),
                    thick,
                    selected,
                );
            }
        }
    }
    // Vertical-axis neighbors: face at v, for each h.
    for v in 0..=sh {
        for h in 0..sw {
            let low = if v == 0 {
                0
            } else {
                label_id(atlas.vol.sample(app.axis, app.index, h, v - 1))
            };
            let high = if v == sh {
                0
            } else {
                label_id(atlas.vol.sample(app.axis, app.index, h, v))
            };
            if low != high && (low != 0 || high != 0) {
                let selected = app
                    .highlighted_region
                    .is_some_and(|id| id == low || id == high);
                let id = if selected {
                    app.highlighted_region.unwrap_or(high)
                } else if high != 0 {
                    high
                } else {
                    low
                };
                let thick = if selected {
                    atlas.width.max(3)
                } else {
                    atlas.width
                };
                stroke_face(
                    &mut strokes,
                    app,
                    sw,
                    sh,
                    (h, v),
                    (h + 1, v),
                    atlas.color(id),
                    thick,
                    selected,
                );
            }
        }
    }
    strokes.blend_into(rgb, channels, atlas.opacity);
}

fn draw_wireframe(
    rgb: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    app: &App,
    atlas: &crate::layers::Atlas,
    coords: &[(usize, usize)],
) {
    use crate::layers::label_id;
    let mut strokes = StrokeCanvas::new(width, height);
    for (idx, &(h, v)) in coords.iter().enumerate() {
        let id = label_id(atlas.vol.sample(app.axis, app.index, h, v));
        if id == 0 {
            continue;
        }
        let x = (idx % width) as i32;
        let y = (idx / width) as i32;
        strokes.plot(x, y, atlas.color(id), 1.0, false);
    }
    // A selected wireframe region can only be identified reliably when
    // companion solid labels are available. Smooth/dilate its raster strokes
    // with the same once-per-pixel compositor used for ordinary atlas edges.
    if let (Some(selected), Some(labels)) = (app.highlighted_region, &atlas.labels) {
        for (idx, &(h, v)) in coords.iter().enumerate() {
            let id = label_id(atlas.vol.sample(app.axis, app.index, h, v));
            if id == 0 {
                continue;
            }
            let region_id = label_id(labels.sample(app.axis, app.index, h, v));
            if region_id != selected {
                continue;
            }
            let x = (idx % width) as i32;
            let y = (idx / width) as i32;
            if app.atlas_aa {
                strokes.soft_stamp(x, y, atlas.color(selected), atlas.width.max(3));
            } else {
                strokes.box_stamp(x, y, atlas.color(selected), atlas.width.max(3), true);
            }
        }
    }
    strokes.blend_into(rgb, channels, atlas.opacity);
}

#[cfg(test)]
mod layer_tests {
    use super::*;
    use crate::{
        layers::{Atlas, IntensityOverlay, Layers},
        volume::Axis,
    };
    use ndarray::Array3;
    use std::collections::HashMap;
    fn volume(a: Vec<f32>) -> Volume {
        Volume {
            data: Array3::from_shape_vec((3, 1, 1), a).unwrap(),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        }
    }
    #[test]
    fn overlays_are_transparent_below_min_and_ordered() {
        let base = volume(vec![100., 100., 100.]);
        let top = volume(vec![0., 2., 9.]);
        let atlas = volume(vec![0., 0., 0.]);
        let layers = Layers {
            overlays: vec![IntensityOverlay {
                vol: top,
                window: (2., 9.),
                default_window: (2., 9.),
                data_range: (0., 9.),
                palette: crate::layers::Palette::Solid([255, 0, 0]),
                visible: true,
                opacity: 1.0,
                name: "top".into(),
            }],
            atlas: Some(Atlas {
                vol: atlas,
                wireframe: true,
                labels: None,
                fallback: [0, 255, 0],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 255.), "test".into());
        let rgb = composite(&base, &app, &layers, 3, 1);
        assert_eq!(&rgb[0..4], &[100, 100, 100, 255]);
        assert_eq!(&rgb[4..8], &[100, 100, 100, 255]);
        assert_eq!(&rgb[8..12], &[255, 0, 0, 255]);
        app.show_overlays = false;
        assert_eq!(
            &composite(&base, &app, &layers, 3, 1)[8..12],
            &[100, 100, 100, 255]
        );
    }
    #[test]
    fn reordered_base_can_render_above_overlay_without_losing_atlas() {
        let base = volume(vec![5., 5., 5.]);
        let label = volume(vec![0., 7., 0.]);
        let mut layers = Layers {
            overlays: vec![IntensityOverlay {
                vol: volume(vec![9., 9., 9.]),
                window: (0., 10.),
                default_window: (0., 10.),
                data_range: (0., 9.),
                palette: crate::layers::Palette::Solid([255, 0, 0]),
                visible: true,
                opacity: 1.0,
                name: "red".into(),
            }],
            atlas: Some(Atlas {
                vol: label,
                wireframe: true,
                labels: None,
                fallback: [0, 255, 0],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 10.), "base".into());
        app.sync_layer_order(layers.overlays.len());
        let initial = composite_format(&base, &app, &layers, 3, 1, false);
        assert_eq!(&initial[0..3], &[255, 0, 0]); // overlay on top
        assert_eq!(&initial[3..6], &[0, 255, 0]); // atlas above both

        // Base image becomes highest intensity layer; atlas remains uppermost.
        app.layer_order.swap(0, 1);
        let reordered = composite_format(&base, &app, &layers, 3, 1, false);
        assert_eq!(&reordered[0..3], &[128, 128, 128]); // base above overlay
        assert_eq!(&reordered[3..6], &[0, 255, 0]);
        assert_eq!(layers.overlays[0].window, (0., 10.));

        // Hidden overlay stays hidden after moving it.
        layers.overlays[0].visible = false;
        let hidden = composite_format(&base, &app, &layers, 3, 1, false);
        assert_eq!(&hidden[0..3], &[128, 128, 128]);
    }

    #[test]
    fn rgb_mode_blends_overlays_but_uses_opaque_black_for_empty_voxels() {
        let base = volume(vec![0., 10., 10.]);
        let top = volume(vec![0., 0., 5.]);
        let layers = Layers {
            overlays: vec![IntensityOverlay {
                vol: top,
                window: (0., 10.),
                default_window: (0., 10.),
                data_range: (0., 5.),
                palette: crate::layers::Palette::Solid([255, 0, 0]),
                visible: true,
                opacity: 0.5,
                name: "top".into(),
            }],
            atlas: None,
        };
        let app = App::new(Axis::Z, 0, 1, 0, (0., 10.), "base".into());
        let rgb = composite_format(&base, &app, &layers, 3, 1, false);
        assert_eq!(rgb.len(), 9);
        assert_eq!(&rgb[..3], &[0, 0, 0]);
        assert_eq!(&rgb[3..6], &[255, 255, 255]);
        assert_eq!(&rgb[6..9], &[255, 128, 128]);
        let rgba = composite_format(&base, &app, &layers, 3, 1, true);
        assert_eq!(&rgba[..4], &[0, 0, 0, 0]);
        assert_eq!(&rgba[8..12], &[255, 128, 128, 255]);
    }
    #[test]
    fn base_and_overlay_below_threshold_leave_transparent_alpha() {
        let base = volume(vec![0.0, 1.0, 2.0]);
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 2.), "test".into());
        app.base_palette = crate::layers::Palette::Warm;
        let pixels = composite(&base, &app, &Layers::default(), 3, 1);
        assert_eq!(&pixels[..4], &[0, 0, 0, 0]);
        assert_eq!(&pixels[4..8], &[255, 128, 0, 255]);
        app.base_opacity = 0.5;
        let pixels = composite(&base, &app, &Layers::default(), 3, 1);
        assert_eq!(pixels[7], 128);
    }
    #[test]
    fn atlas_alpha_is_independent_and_topmost() {
        let base = volume(vec![0.0, 0.0, 0.0]);
        let labels = volume(vec![7.0, 0.0, 0.0]);
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: labels,
                wireframe: true,
                labels: None,
                fallback: [120, 200, 250],
                width: 1,
                opacity: 0.5,
                regions: HashMap::new(),
            }),
        };
        let app = App::new(Axis::Z, 0, 1, 0, (0.0, 1.0), "base".into());
        let pixels = composite(&base, &app, &layers, 3, 1);
        assert_eq!(&pixels[..4], &[120, 200, 250, 128]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0]);
    }
    #[test]
    fn hovered_region_gets_thicker_edges_without_changing_other_pixels() {
        let base = volume(vec![0., 0., 0.]);
        let atlas = volume(vec![3., 3., 4.]);
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: atlas,
                wireframe: false,
                labels: None,
                fallback: [255, 0, 255],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "base".into());
        let normal = composite_format(&base, &app, &layers, 30, 10, false);
        app.highlighted_region = Some(3);
        let highlighted = composite_format(&base, &app, &layers, 30, 10, false);
        // The edge x=20 between labels 3 and 4 grows to x=19 when selected.
        assert_eq!(
            &normal[(5 * 30 + 19) * 3..(5 * 30 + 19) * 3 + 3],
            &[0, 0, 0]
        );
        assert_eq!(
            &highlighted[(5 * 30 + 19) * 3..(5 * 30 + 19) * 3 + 3],
            &[255, 0, 255]
        );
    }
    #[test]
    fn intersecting_edges_do_not_accumulate_opacity() {
        // Four labeled regions form a cross at a voxel-face intersection.
        // A single 50%-opaque stroke and its 4-way junction must be equally
        // bright: drawing/alpha-blending every face separately brightened it.
        let base = Volume {
            data: Array3::zeros((4, 4, 1)),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let labels = Volume {
            data: Array3::from_shape_fn((4, 4, 1), |(x, y, _)| match (x < 2, y < 2) {
                (true, true) => 1.0,
                (false, true) => 2.0,
                (true, false) => 3.0,
                (false, false) => 4.0,
            }),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: labels,
                wireframe: false,
                labels: None,
                fallback: [255, 0, 255],
                width: 1,
                opacity: 0.5,
                regions: HashMap::new(),
            }),
        };
        let app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "base".into());
        let raster = composite_format(&base, &app, &layers, 40, 40, false);
        let at = |x: usize, y: usize| (y * 40 + x) * 3;
        assert_eq!(&raster[at(20, 20)..at(20, 20) + 3], &[128, 0, 128]);
        assert_eq!(
            &raster[at(20, 15)..at(20, 15) + 3],
            &raster[at(20, 20)..at(20, 20) + 3]
        );
        assert_eq!(
            &raster[at(15, 20)..at(15, 20) + 3],
            &raster[at(20, 20)..at(20, 20) + 3]
        );
    }

    #[test]
    fn antialiasing_applies_to_thick_hovered_region() {
        let base = volume(vec![0., 0., 0.]);
        let atlas = volume(vec![1., 1., 2.]);
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: atlas,
                wireframe: false,
                labels: None,
                fallback: [255, 0, 255],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "base".into());
        app.highlighted_region = Some(1);
        let sharp = composite_format(&base, &app, &layers, 29, 10, false);
        app.atlas_aa = true;
        let smooth = composite_format(&base, &app, &layers, 29, 10, false);
        assert_ne!(sharp, smooth);
        assert!(smooth
            .chunks_exact(3)
            .any(|pixel| pixel[0] > 0 && pixel[0] < 255));
    }

    #[test]
    fn atlas_aa_is_opt_in_and_produces_subpixel_coverage() {
        let base = volume(vec![0., 0., 0.]);
        let atlas = volume(vec![1., 1., 2.]);
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: atlas,
                wireframe: false,
                labels: None,
                fallback: [255, 0, 255],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "base".into());
        // 3 voxel columns projected into 8 pixels puts one boundary at x=5.333.
        let exact = composite_format(&base, &app, &layers, 8, 6, false);
        app.atlas_aa = true;
        let soft = composite_format(&base, &app, &layers, 8, 6, false);
        assert_ne!(exact, soft);
        assert!(soft.chunks_exact(3).any(|px| px[0] > 0 && px[0] < 255));
    }
    #[test]
    fn boundaries_project_to_faces_not_voxel_centers() {
        let base = volume(vec![0., 0., 0.]);
        let label = volume(vec![3., 3., 4.]);
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: label,
                wireframe: false,
                labels: None,
                fallback: [255, 0, 255],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "test".into());
        let rgb = composite(&base, &app, &layers, 30, 10);
        // Internal boundary between source h=1 and h=2 -> x=20, not center x=15 or 25.
        assert_eq!(
            &rgb[(5 * 30 + 20) * 4..(5 * 30 + 20) * 4 + 3],
            &[255, 0, 255]
        );
        assert_eq!(&rgb[(5 * 30 + 15) * 4..(5 * 30 + 15) * 4 + 3], &[0, 0, 0]);
    }
}

#[cfg(test)]
mod inspector_tests {
    use super::*;
    use crate::volume::Axis;
    use ndarray::Array3;
    #[test]
    fn inverse_mapping_respects_rotation_and_flips() {
        let vol = Volume {
            data: Array3::zeros((4, 3, 2)),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let mut app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "test".into());
        assert_eq!(voxel_at_raster(&vol, &app, 0, 0, 4, 3), Some((0, 2)));
        assert_eq!(voxel_at_raster(&vol, &app, 3, 2, 4, 3), Some((3, 0)));
        app.rotation = 90;
        assert_eq!(voxel_at_raster(&vol, &app, 0, 0, 3, 4), Some((0, 0)));
        assert_eq!(voxel_at_raster(&vol, &app, 2, 3, 3, 4), Some((3, 2)));
        app.flip_h = true;
        assert_eq!(voxel_at_raster(&vol, &app, 0, 0, 3, 4), Some((0, 2)));
    }
    #[test]
    fn exact_contours_do_not_erase_internal_region() {
        use crate::layers::{Atlas, Layers};
        use std::collections::HashMap;
        let base = Volume {
            data: Array3::zeros((3, 3, 1)),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let label = Volume {
            data: Array3::from_shape_fn(
                (3, 3, 1),
                |(x, y, _)| if x == 1 && y == 1 { 3. } else { 4. },
            ),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let layers = Layers {
            overlays: vec![],
            atlas: Some(Atlas {
                vol: label,
                wireframe: false,
                labels: None,
                fallback: [255, 0, 255],
                width: 1,
                opacity: 1.0,
                regions: HashMap::new(),
            }),
        };
        let app = App::new(Axis::Z, 0, 1, 0, (0., 1.), "test".into());
        let rgb = composite(&base, &app, &layers, 90, 90);
        // Center of one-voxel island remains unobstructed by contours.
        assert_eq!(&rgb[(45 * 90 + 45) * 4..(45 * 90 + 45) * 4 + 3], &[0, 0, 0]);
        assert!(rgb.iter().any(|&v| v > 0));
    }
}
