use crossterm::terminal;
use std::collections::HashMap;
use crate::config::AtlasStyle;

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
pub fn geometry(vol: &Volume, app: &App, term_cols: u16, term_rows: u16, max_pixels: usize, reserved_rows: usize) -> Geometry {
    let (_, _, mut aspect) = vol.plane(app.axis);
    if app.rotation % 180 != 0 { aspect = 1.0 / aspect; }
    let usable_cols = term_cols.saturating_sub(1).max(1) as usize;
    let usable_rows = (term_rows as usize).saturating_sub(reserved_rows).max(1);
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
                match app.atlas_style {
                    AtlasStyle::Smooth => draw_atlas_smooth(&mut rgb, width, height, app, atlas, sw, sh),
                    AtlasStyle::Voxel => draw_atlas_edges(&mut rgb, width, height, app, atlas, sw, sh),
                }
            }
        }
    }
    if app.inspect_mode {
        if let Some((h,v)) = app.probe {
            if h < sw && v < sh {
                draw_inspector_mark(&mut rgb, width, height, app, sw, sh, h, v);
            }
        }
    }
    rgb
}

/// Inverse display mapping. The exact same mapping as the grayscale raster.
/// Coordinates here refer to raster pixels (not terminal cells).
pub fn voxel_at_raster(vol: &Volume, app: &App, x: usize, y: usize,
    width: usize, height: usize) -> Option<(usize,usize)> {
    if x >= width || y >= height || width == 0 || height == 0 { return None; }
    let (sw,sh,_) = vol.plane(app.axis);
    let swapped = app.rotation % 180 != 0;
    let ow = if swapped { sh } else { sw };
    let oh = if swapped { sw } else { sh };
    let mut ox = (x*ow/width).min(ow-1);
    let mut oy = (y*oh/height).min(oh-1);
    if app.flip_h { ox=ow-1-ox; }
    if app.flip_v { oy=oh-1-oy; }
    Some(inverse_rotation(ox,oy,sw,sh,app.rotation))
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


/// Subpixel projection of voxel FACE positions. Rounding is deferred to the
/// antialiased renderer, unlike the exact voxel-edge renderer above.
fn project_subpixel(app: &App, h: f32, v: f32, sw: usize, sh: usize,
    width: usize, height: usize) -> (f32, f32) {
    let sw=sw as f32;
    let sh=sh as f32;
    let (mut x,mut y,ow,oh) = match app.rotation {
        0 => (h,sh-v,sw,sh),
        90 => (v,h,sh,sw),
        180 => (sw-h,v,sw,sh),
        270 => (sh-v,sw-h,sh,sw),
        _ => unreachable!(),
    };
    if app.flip_h { x=ow-x; }
    if app.flip_v { y=oh-y; }
    (x*width as f32/ow,y*height as f32/oh)
}

fn draw_inspector_mark(rgb: &mut [u8], width: usize, height: usize,
    app: &App, sw: usize, sh: usize, h: usize, v: usize) {
    let (x,y) = project_subpixel(app,h as f32+0.5,v as f32+0.5,sw,sh,width,height);
    let x=x.round() as i32;
    let y=y.round() as i32;
    for d in -4..=4 {
        paint(rgb,width,height,x+d,y,[0,0,0],3);
        paint(rgb,width,height,x,y+d,[0,0,0],3);
    }
    for d in -3..=3 {
        paint(rgb,width,height,x+d,y,[255,255,255],1);
        paint(rgb,width,height,x,y+d,[255,255,255],1);
    }
}

#[derive(Clone,Copy)]
struct FaceEdge {
    a: (usize,usize), b: (usize,usize), pair: (i32,i32), color: [u8;3],
}

fn atlas_faces(app: &App, atlas: &crate::layers::Atlas, sw: usize, sh: usize) -> Vec<FaceEdge> {
    use crate::layers::label_id;
    let mut edges = Vec::new();
    // Horizontal-axis neighbors: face at h, for each v.
    for v in 0..sh {
        for h in 0..=sw {
            let left = if h == 0 { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h-1,v)) };
            let right = if h == sw { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h,v)) };
            if left != right {
                let id = left.max(right);
                edges.push(FaceEdge { a:(h,v),b:(h,v+1),pair:(left.min(right),id),color:atlas.color(id) });
            }
        }
    }
    // Vertical-axis neighbors: face at v, for each h.
    for v in 0..=sh {
        for h in 0..sw {
            let low = if v == 0 { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h,v-1)) };
            let high = if v == sh { 0 } else { label_id(atlas.vol.sample(app.axis,app.index,h,v)) };
            if low != high {
                let id = low.max(high);
                edges.push(FaceEdge { a:(h,v),b:(h+1,v),pair:(low.min(high),id),color:atlas.color(id) });
            }
        }
    }
    edges
}

/// Draw an antialiased line, blended over the already-composited intensities.
/// Local rectangular coverage makes it significantly cheaper than rendering
/// the whole atlas at 4x supersampling and downsampling every frame.
fn aa_line(rgb: &mut [u8], width: usize, height: usize,
    p: (f32,f32), q: (f32,f32), color: [u8;3], stroke: f32) {
    let radius=stroke*0.5;
    let x0=((p.0.min(q.0)-radius-1.).floor() as i32).max(0) as usize;
    let x1=((p.0.max(q.0)+radius+1.).ceil() as i32).min(width as i32-1).max(-1);
    let y0=((p.1.min(q.1)-radius-1.).floor() as i32).max(0) as usize;
    let y1=((p.1.max(q.1)+radius+1.).ceil() as i32).min(height as i32-1).max(-1);
    if x1 < x0 as i32 || y1 < y0 as i32 { return; }
    let dx=q.0-p.0; let dy=q.1-p.1;
    let len2=dx*dx+dy*dy;
    for y in y0..=y1 as usize {
        for x in x0..=x1 as usize {
            let u=x as f32+0.5-p.0;
            let v=y as f32+0.5-p.1;
            let t=if len2>1e-6 { ((u*dx+v*dy)/len2).clamp(0.,1.) } else { 0. };
            let ex=u-t*dx; let ey=v-t*dy;
            let dist=(ex*ex+ey*ey).sqrt();
            let alpha=(radius+0.5-dist).clamp(0.,1.);
            if alpha==0. { continue; }
            let px=(y*width+x)*3;
            for c in 0..3 {
                rgb[px+c] = (rgb[px+c] as f32*(1.-alpha)+color[c] as f32*alpha).round() as u8;
            }
        }
    }
}

fn lerp(a: (f32,f32), b: (f32,f32), t: f32) -> (f32,f32) {
    (a.0+(b.0-a.0)*t,a.1+(b.1-a.1)*t)
}

/// Draw quadratic Bezier with short antialiased linelets. Used at unambiguous
/// bends only; multi-region junctions remain unmodified.
fn aa_quad(rgb: &mut [u8], width: usize, height: usize,
    a: (f32,f32), corner: (f32,f32), b: (f32,f32), color:[u8;3], stroke:f32) {
    let length=((a.0-corner.0).hypot(a.1-corner.1)
        +(b.0-corner.0).hypot(b.1-corner.1)).max(1.);
    let n=(length/2.0).ceil().clamp(3.,32.) as usize;
    let mut previous=a;
    for j in 1..=n {
        let t=j as f32/n as f32;
        let u=1.-t;
        let next=(u*u*a.0+2.*u*t*corner.0+t*t*b.0,
                  u*u*a.1+2.*u*t*corner.1+t*t*b.1);
        aa_line(rgb,width,height,previous,next,color,stroke);
        previous=next;
    }
}

/// Fast "rounded" anatomical boundaries: extract exact voxel-face edges, then
/// antialias them and round only ordinary degree-two corners. The radius is
/// 0.22 source voxel, so small one-voxel structures remain present. Color
/// junctions are not smoothed across unrelated IDs.
fn draw_atlas_smooth(rgb: &mut [u8], width: usize, height: usize,
    app: &App, atlas: &crate::layers::Atlas, sw: usize, sh: usize) {
    let edges=atlas_faces(app,atlas,sw,sh);
    // Key: source-grid vertex + unordered pair of adjacent region IDs.
    // This preserves color consistency when an outline turns a corner.
    // Only smooth unbranched
    // two-edge corners, keeping X/T junctions and color transitions exact.
    let mut at_vertex: HashMap<(usize,usize,i32,i32),Vec<usize>>=HashMap::new();
    for (i,e) in edges.iter().enumerate() {
        let (left,right)=e.pair;
        at_vertex.entry((e.a.0,e.a.1,left,right)).or_default().push(i);
        at_vertex.entry((e.b.0,e.b.1,left,right)).or_default().push(i);
    }
    let rounded=|v:(usize,usize), edge_index:usize, edge:&FaceEdge| -> bool {
        let Some(indices) = at_vertex.get(&(v.0,v.1,edge.pair.0,edge.pair.1)) else { return false; };
        if indices.len()!=2 { return false; }
        let other=&edges[if indices[0]==edge_index { indices[1] } else { indices[0] }];
        // Only perpendicular segments create a corner; the same-color
        // continuation remains straight.
        (edge.a.0==edge.b.0) != (other.a.0==other.b.0)
    };
    const CUT: f32=0.22;
    let stroke=atlas.width as f32;
    for (i,e) in edges.iter().enumerate() {
        let a=(e.a.0 as f32,e.a.1 as f32);
        let b=(e.b.0 as f32,e.b.1 as f32);
        let trim_a=if rounded(e.a,i,e) { CUT } else { 0. };
        let trim_b=if rounded(e.b,i,e) { CUT } else { 0. };
        let pa=lerp(a,b,trim_a);
        let pb=lerp(b,a,trim_b);
        let p=project_subpixel(app,pa.0,pa.1,sw,sh,width,height);
        let q=project_subpixel(app,pb.0,pb.1,sw,sh,width,height);
        aa_line(rgb,width,height,p,q,e.color,stroke);
    }
    // Connect trimmed segments via a quadratic curve at each corner.
    // Each vertex is processed exactly once for a given color.
    for ((x,y,_,_),indices) in &at_vertex {
        if indices.len()!=2 { continue; }
        let e1=&edges[indices[0]]; let e2=&edges[indices[1]];
        if (e1.a.0==e1.b.0)==(e2.a.0==e2.b.0) { continue; }
        let v=(*x as f32,*y as f32);
        let far=|e:&FaceEdge| -> (f32,f32) {
            if e.a==(*x,*y) { (e.b.0 as f32,e.b.1 as f32) }
            else { (e.a.0 as f32,e.a.1 as f32) }
        };
        let p=lerp(v,far(e1),CUT);
        let q=lerp(v,far(e2),CUT);
        aa_quad(rgb,width,height,
            project_subpixel(app,p.0,p.1,sw,sh,width,height),
            project_subpixel(app,v.0,v.1,sw,sh,width,height),
            project_subpixel(app,q.0,q.1,sw,sh,width,height),e1.color,stroke);
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
            vol:top, window:(2., 9.), color:[255,0,0], opacity:1.0, name:"top".into() }],
            atlas: Some(Atlas { vol: atlas, wireframe:true, labels:None, fallback:[0,255,0],width:1,regions:HashMap::new() }) };
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
            vol:label, wireframe:false,labels:None,fallback:[255,0,255],width:1,regions:HashMap::new()
        }) };
        let mut app = App::new(Axis::Z,0,1,0,(0.,1.),"test".into());
        app.atlas_style = AtlasStyle::Voxel;
        let rgb = composite(&base,&app,&layers,30,10);
        // Internal boundary between source h=1 and h=2 -> x=20, not center x=15 or 25.
        assert_eq!(&rgb[(5*30+20)*3..(5*30+20)*3+3], &[255,0,255]);
        assert_eq!(&rgb[(5*30+15)*3..(5*30+15)*3+3], &[0,0,0]);
    }
}

#[cfg(test)]
mod inspector_tests {
    use super::*;
    use crate::volume::Axis;
    use ndarray::Array3;
    #[test]
    fn inverse_mapping_respects_rotation_and_flips() {
        let vol=Volume {data:Array3::zeros((4,3,2)),spacing:[1.;3],sform:None};
        let mut app=App::new(Axis::Z,0,1,0,(0.,1.),"test".into());
        assert_eq!(voxel_at_raster(&vol,&app,0,0,4,3),Some((0,2)));
        assert_eq!(voxel_at_raster(&vol,&app,3,2,4,3),Some((3,0)));
        app.rotation=90;
        assert_eq!(voxel_at_raster(&vol,&app,0,0,3,4),Some((0,0)));
        assert_eq!(voxel_at_raster(&vol,&app,2,3,3,4),Some((3,2)));
        app.flip_h=true;
        assert_eq!(voxel_at_raster(&vol,&app,0,0,3,4),Some((0,2)));
    }
    #[test]
    fn smooth_contours_do_not_erase_internal_region() {
        use crate::layers::{Atlas,Layers};
        use std::collections::HashMap;
        let base=Volume {data:Array3::zeros((3,3,1)),spacing:[1.;3],sform:None};
        let label=Volume {data:Array3::from_shape_fn((3,3,1),|(x,y,_)|
            if x==1 && y==1 {3.} else {4.}),spacing:[1.;3],sform:None};
        let layers=Layers {overlays:vec![],atlas:Some(Atlas {vol:label,wireframe:false,labels:None,
            fallback:[255,0,255],width:1,regions:HashMap::new()})};
        let app=App::new(Axis::Z,0,1,0,(0.,1.),"test".into());
        let rgb=composite(&base,&app,&layers,90,90);
        // Center of one-voxel island remains unobstructed by contours.
        assert_eq!(&rgb[(45*90+45)*3..(45*90+45)*3+3],&[0,0,0]);
        assert!(rgb.iter().any(|&v|v>0));
    }
}
