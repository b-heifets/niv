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
            spacing: [1.;3], sform: None,
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

/// Composite all intensity volumes bottom-to-top, then the atlas on top.
/// The atlas and overlays are required to be on precisely the same voxel grid.
pub fn composite(vol: &Volume, app: &App, layers: &crate::layers::Layers,
    width: usize, height: usize) -> Vec<u8> {
    let mut rgb = grayscale(vol, app, width, height);
    let (sw, sh, _) = vol.plane(app.axis);
    let swapped = app.rotation % 180 != 0;
    let oriented_w = if swapped { sh } else { sw };
    let oriented_h = if swapped { sw } else { sh };
    let horizontal: Vec<usize> = (0..width).map(|x| {
        let h = (x * oriented_w / width).min(oriented_w-1);
        if app.flip_h { oriented_w - 1 - h } else { h }
    }).collect();
    let vertical: Vec<usize> = (0..height).map(|y| {
        let v = (y * oriented_h / height).min(oriented_h-1);
        if app.flip_v { oriented_h - 1 - v } else { v }
    }).collect();

    if app.show_overlays {
        for overlay in &layers.overlays {
            let (lo, hi) = overlay.window;
            for (y, row) in rgb.chunks_exact_mut(width * 3).enumerate() {
                for (x, px) in row.chunks_exact_mut(3).enumerate() {
                    let (h, v) = inverse_rotation(horizontal[x], vertical[y], sw, sh, app.rotation);
                    let value = overlay.vol.sample(app.axis, app.index, h, v);
                    // Values <= lower display bound are truly transparent.
                    if !value.is_finite() || value <= lo { continue; }
                    let strength = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
                    let alpha = overlay.opacity * strength;
                    for c in 0..3 {
                        px[c] = (px[c] as f32 * (1.0-alpha) +
                            overlay.color[c] as f32 * alpha).round() as u8;
                    }
                }
            }
        }
    }

    if app.show_atlas {
        if let Some(atlas) = &layers.atlas {
            if atlas.wireframe {
                // A precomputed wireframe already marks voxels. Its IDs are
                // used directly for colors; 1 means binary-only wireframe.
                for (y, row) in rgb.chunks_exact_mut(width * 3).enumerate() {
                    for (x, px) in row.chunks_exact_mut(3).enumerate() {
                        let (h, v) = inverse_rotation(horizontal[x], vertical[y], sw, sh, app.rotation);
                        let id = crate::layers::label_id(atlas.vol.sample(app.axis, app.index, h, v));
                        if id > 0 { px.copy_from_slice(&atlas.color(id)); }
                    }
                }
            } else {
                draw_atlas_edges(&mut rgb, width, height, app, atlas, sw, sh);
            }
        }
    }
    rgb
}

#[inline]
fn inverse_rotation(x: usize, y: usize, sw: usize, sh: usize, rotate: u16) -> (usize,usize) {
    match rotate {
        0 => (x, sh-1-y),
        90 => (y, x),
        180 => (sw-1-x, y),
        270 => (sw-1-y, sh-1-x),
        _ => unreachable!(),
    }
}

/// Map integer source voxel FACE coordinates to output pixel edges.
/// The atlas is drawn AFTER all intensity overlays and therefore cannot be
/// occluded by any of them. Edges stay between the nearest-neighbor cells.
fn project(app: &App, h: usize, v: usize, sw: usize, sh: usize,
    width: usize, height: usize) -> (i32, i32) {
    let (mut x, mut y, ow, oh) = match app.rotation {
        0 => (h, sh-v, sw, sh),
        90 => (v, h, sh, sw),
        180 => (sw-h, v, sw, sh),
        270 => (sh-v, sw-h, sh, sw),
        _ => unreachable!(),
    };
    if app.flip_h { x = ow - x; }
    if app.flip_v { y = oh - y; }
    let px = ((x as f64 * width as f64 / ow as f64).round() as i32)
        .clamp(0, width as i32 - 1);
    let py = ((y as f64 * height as f64 / oh as f64).round() as i32)
        .clamp(0, height as i32 - 1);
    (px, py)
}

fn paint(rgb: &mut [u8], width: usize, height: usize, x: i32, y: i32,
    color: [u8; 3], thick: usize) {
    // Even widths extend toward +x/+y, odd widths centered.
    let neg = ((thick - 1) / 2) as i32;
    for yy in y-neg..y-neg+thick as i32 {
        for xx in x-neg..x-neg+thick as i32 {
            if xx >= 0 && yy >= 0 && (xx as usize) < width && (yy as usize) < height {
                let off = (yy as usize * width + xx as usize) * 3;
                rgb[off..off+3].copy_from_slice(&color);
            }
        }
    }
}

fn draw_face(rgb: &mut [u8], width: usize, height: usize, app: &App,
    sw: usize, sh: usize, p: (usize, usize), q: (usize, usize),
    color: [u8; 3], thick: usize) {
    let (x0, y0) = project(app, p.0, p.1, sw, sh, width, height);
    let (x1, y1) = project(app, q.0, q.1, sw, sh, width, height);
    if x0 == x1 {
        for y in y0.min(y1)..=y0.max(y1) { paint(rgb,width,height,x0,y,color,thick); }
    } else {
        for x in x0.min(x1)..=x0.max(x1) { paint(rgb,width,height,x,y0,color,thick); }
    }
}

fn draw_atlas_edges(rgb: &mut [u8], width: usize, height: usize, app: &App,
    atlas: &crate::layers::Atlas, sw: usize, sh: usize) {
    use crate::layers::label_id;
    // Horizontal-axis neighbors: draw face at h, for each v.
    for v in 0..sh {
        for h in 0..=sw {
            let left = if h == 0 { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h-1,v)) };
            let right = if h == sw { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h,v)) };
            if left != right && (left != 0 || right != 0) {
                let id = if right != 0 { right } else { left };
                draw_face(rgb,width,height,app,sw,sh,(h,v),(h,v+1),atlas.color(id),atlas.width);
            }
        }
    }
    // Vertical-axis neighbors: face at v, for each h.
    for v in 0..=sh {
        for h in 0..sw {
            let low = if v == 0 { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h,v-1)) };
            let high = if v == sh { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h,v)) };
            if low != high && (low != 0 || high != 0) {
                let id = if high != 0 { high } else { low };
                draw_face(rgb,width,height,app,sw,sh,(h,v),(h+1,v),atlas.color(id),atlas.width);
            }
        }
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;
    use crate::{layers::{Atlas, IntensityOverlay, Layers}, volume::Axis};
    use ndarray::Array3;
    use std::collections::HashMap;
    fn volume(a: Vec<f32>) -> Volume {
        Volume { data: Array3::from_shape_vec((3,1,1),a).unwrap(), spacing:[1.;3], sform:None }
    }
    #[test]
    fn overlays_are_transparent_below_min_and_ordered() {
        let base = volume(vec![100.,100.,100.]);
        let top = volume(vec![0., 2., 9.]);
        let atlas = volume(vec![0., 0., 0.]);
        let layers = Layers { overlays: vec![IntensityOverlay {
            vol:top, window:(2., 9.), color:[255,0,0], opacity:1.0 }],
            atlas: Some(Atlas { vol: atlas, wireframe:true, fallback:[0,255,0],width:1,regions:HashMap::new() }) };
        let mut app = App::new(Axis::Z,0,1,0,(0.,255.),"test".into());
        let rgb = composite(&base,&app,&layers,3,1);
        assert_eq!(&rgb[0..3], &[100,100,100]);
        assert_eq!(&rgb[3..6], &[100,100,100]);
        assert_eq!(&rgb[6..9], &[255,0,0]);
        app.show_overlays=false;
        assert_eq!(&composite(&base,&app,&layers,3,1)[6..9], &[100,100,100]);
    }
    #[test]
    fn boundaries_project_to_faces_not_voxel_centers() {
        let base=volume(vec![0.,0.,0.]);
        let label=volume(vec![3.,3.,4.]);
        let layers=Layers { overlays:vec![], atlas:Some(Atlas {
            vol:label, wireframe:false,fallback:[255,0,255],width:1,regions:HashMap::new()
        }) };
        let app = App::new(Axis::Z,0,1,0,(0.,1.),"test".into());
        let rgb = composite(&base,&app,&layers,30,10);
        // Internal boundary between source h=1 and h=2 -> x=20, not center x=15 or 25.
        assert_eq!(&rgb[(5*30+20)*3..(5*30+20)*3+3], &[255,0,255]);
        assert_eq!(&rgb[(5*30+15)*3..(5*30+15)*3+3], &[0,0,0]);
    }
}
