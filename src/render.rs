use crossterm::terminal;

use crate::{app::App, volume::Volume};

#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub width: usize,
    pub height: usize,
}

/// Uses terminal pixel-size reporting if available; falls back to 8x16 pixels/cell.
/// Render at the target pixel dimensions unless the max-pixels budget would be exceeded.
pub fn geometry(vol: &Volume, app: &App, term_cols: u16, term_rows: u16, max_pixels: usize) -> Geometry {
    let (_, _, aspect) = vol.plane(app.axis);
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

/// RGB24 raster. All interpolation is nearest neighbor. The default vertical
/// direction draws increasing voxel indices upwards; flipping reverses this.
pub fn grayscale(vol: &Volume, app: &App, width: usize, height: usize) -> Vec<u8> {
    let (source_w, source_h, _) = vol.plane(app.axis);
    let mut raster = vec![0u8; width * height * 3];
    let (lo, hi) = app.window;
    let gain = 255.0 / (hi - lo).max(f32::MIN_POSITIVE);

    // Precompute the nearest-neighbor coordinates so the loop only samples and maps values.
    let horizontal: Vec<usize> = (0..width).map(|x| {
        let index = (x * source_w / width).min(source_w - 1);
        if app.flip_h { source_w - index - 1 } else { index }
    }).collect();
    let vertical: Vec<usize> = (0..height).map(|y| {
        let index = (y * source_h / height).min(source_h - 1);
        if app.flip_v { index } else { source_h - index - 1 }
    }).collect();

    for (y, row) in raster.chunks_exact_mut(width * 3).enumerate() {
        let v = vertical[y];
        for (x, px) in row.chunks_exact_mut(3).enumerate() {
            let intensity = vol.sample(app.axis, app.index, horizontal[x], v);
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
    fn horizontal_and_vertical_flips() {
        let vol = Volume { data: Array3::from_shape_fn((2,2,1), |(x,y,_)| (10*x+100*y) as f32), spacing: [1.;3] };
        let mut app = App::new(Axis::Z, 0, 1, (0., 110.), "test".into());
        let first = grayscale(&vol, &app, 2, 2);
        assert_eq!(first[0], 231); // (x=0,y=1): 100*255/110
        assert_eq!(first[3], 255); // (x=1,y=1): 110
        app.flip_h = true;
        let flipped = grayscale(&vol, &app, 2, 2);
        assert_eq!(flipped[0], 255);
        app.flip_v = true;
        let both = grayscale(&vol, &app, 2, 2);
        assert_eq!(both[0], 23); // (x=1,y=0): 10*255/110
    }
}
