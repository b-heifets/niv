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
pub fn geometry(vol: &Volume, app: &App, term_cols: u16, term_rows: u16, max_pixels: usize) -> Geometry {
    let (_, _, mut aspect) = vol.plane(app.axis);
    if app.rotation % 180 != 0 { aspect = 1.0 / aspect; }
    let usable_cols = term_cols.saturating_sub(1).max(1) as usize;
    let usable_rows = term_rows.saturating_sub(4).max(1) as usize;
    let (cell_w, cell_h) = cell_pixels(term_cols, term_rows);
    let viewport_w = usable_cols * cell_w;
    let viewport_h = usable_rows * cell_h;

    let (wanted_w, wanted_h) = if viewport_w as f64 / viewport_h as f64 > aspect {
        ((viewport_h as f64 * aspect).round().max(1.) as usize, viewport_h)
    } else {
        (viewport_w, (viewport_w as f64 / aspect).round().max(1.) as usize)
    };
    let cols = ((wanted_w / cell_w).max(1)).min(usable_cols) as u16;
    let rows = ((wanted_h / cell_h).max(1)).min(usable_rows) as u16;
    let view_w = cols as usize * cell_w;
    let view_h = rows as usize * cell_h;
    let scale = (max_pixels as f64 / (view_w * view_h).max(1) as f64).sqrt().min(1.0);
    Geometry {
        cols,
        rows,
        width: ((view_w as f64 * scale).round() as usize).max(1),
        height: ((view_h as f64 * scale).round() as usize).max(1),
    }
}

fn cell_pixels(cols: u16, rows: u16) -> (usize, usize) {
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
pub fn grayscale(vol: &Volume, app: &App, width: usize, height: usize) -> Vec<u8> {
    let (source_w, source_h, _) = vol.plane(app.axis);
    let mut raster = vec![0u8; width * height * 3];
    let (lo, hi) = app.window;
    let gain = 255.0 / (hi - lo).max(f32::MIN_POSITIVE);
    let swapped = app.rotation % 180 != 0;
    let oriented_w = if swapped { source_h } else { source_w };
    let oriented_h = if swapped { source_w } else { source_h };
    let horizontal: Vec<usize> = (0..width).map(|x| {
        let index = (x * oriented_w / width).min(oriented_w - 1);
        if app.flip_h { oriented_w - 1 - index } else { index }
    }).collect();
    let vertical: Vec<usize> = (0..height).map(|y| {
        let index = (y * oriented_h / height).min(oriented_h - 1);
        if app.flip_v { oriented_h - 1 - index } else { index }
    }).collect();

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
            let g = if intensity.is_finite() {
                ((intensity - lo) * gain).clamp(0., 255.) as u8
            } else { 0 };
            px.copy_from_slice(&[g, g, g]);
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
    fn flips_and_rotations() {
        // Data contains displayed rows [40,50], [20,30], [0,10] (top to bottom).
        // We compare grayscale red channel at each output pixel.
        let vol = Volume {
            data: Array3::from_shape_fn((2, 3, 1), |(x,y,_)| (10*x + 20*y) as f32),
            spacing: [1.;3],
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
