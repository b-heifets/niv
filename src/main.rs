//! niv — direct Kitty graphics NIfTI slice viewer. No libghostty/Rex dependency.
mod app;
mod config;
mod kitty;
mod layers;
mod render;
mod volume;

use std::{
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use app::{Action, App};
use config::Config;
use layers::Layers;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, MouseEvent, MouseEventKind, EnableMouseCapture,
        DisableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use volume::Volume;

struct TerminalGuard { enhanced_keys: bool, mouse_pixels: bool }
impl TerminalGuard {
    fn enter(enhanced_keys: bool, mouse_pixels: bool) -> Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(err) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(err.into());
        }
        if enhanced_keys {
            // Unsupported terminals should ignore this request. Non-Kitty muxes
            // can opt out with --no-enhanced-keys.
            if let Err(err) = execute!(io::stdout(), PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )) {
                let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
                let _ = terminal::disable_raw_mode();
                return Err(err.into());
            }
        }
        execute!(io::stdout(), EnableMouseCapture)?;
        if mouse_pixels {
            // Requires a terminal/mux that forwards SGR pixel mouse reports.
            io::stdout().write_all(b"\x1b[?1016h")?;
            io::stdout().flush()?;
        }
        Ok(Self { enhanced_keys, mouse_pixels })
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.mouse_pixels {
            let _ = stdout.write_all(b"\x1b[?1016l");
        }
        let _ = execute!(stdout, DisableMouseCapture);
        if self.enhanced_keys {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn crop(s: String, cols: u16) -> String {
    s.chars().take(cols as usize).collect()
}

/// Owns one active Kitty image and allocates a fresh ID for every frame.
/// This is deliberately NOT a fixed-ID re-transmission: Kitty removes old
/// placements when overwriting an existing image ID.
struct FrameState {
    next_id: u32,
    active_id: Option<u32>,
}
impl FrameState {
    fn new() -> Self {
        let pid = std::process::id();
        Self { next_id: 3_000_000u32.wrapping_add(pid.wrapping_mul(10_000)), active_id: None }
    }
    fn allocate(&mut self) -> u32 {
        let id = self.next_id.max(1);
        self.next_id = id.wrapping_add(1).max(1);
        id
    }
    fn clear(&mut self, out: &mut impl Write) -> Result<()> {
        if let Some(id) = self.active_id.take() {
            kitty::delete(out, id)?;
        }
        Ok(())
    }
}


fn intensity(v: f32) -> String {
    if !v.is_finite() { return v.to_string(); }
    if v != 0. && (v.abs() < 0.0001 || v.abs() >= 1e7) {
        format!("{v:.4e}")
    } else { format!("{v:.4}") }
}

/// The footer is the only part repainted during mouse movement. We reserve
/// one line per additional layer so none of its raw voxel values are hidden.
fn footer_lines(vol: &Volume, app: &App, layers: &Layers, data_range: (f32,f32)) -> Vec<String> {
    let mut lines=vec![format!(
        "data {}..{} | display {}..{} | outlines:{}",
        intensity(data_range.0), intensity(data_range.1),
        intensity(app.window.0), intensity(app.window.1),
        match app.atlas_style { config::AtlasStyle::Smooth => "smooth", config::AtlasStyle::Voxel => "voxel" },
    )];
    if let Some((h,v))=app.probe {
        let [x,y,z]=app.axis.xyz(app.index,h,v);
        let tag=if app.inspect_mode { "inspect" } else { "hover" };
        lines.push(format!("{tag} [{x}, {y}, {z}] | base {}: {}",
            app.path, intensity(vol.sample(app.axis,app.index,h,v))));
        for (i,layer) in layers.overlays.iter().enumerate() {
            lines.push(format!("layer {} {}{}: {} | window {}..{}",
                i+1,layer.name,if app.show_overlays { "" } else { " (hidden)" },
                intensity(layer.vol.sample(app.axis,app.index,h,v)),
                intensity(layer.window.0),intensity(layer.window.1)));
        }
        if layers.atlas.is_some() {
            let region=layers.region_at(app.axis,app.index,h,v).unwrap_or_else(||
                "region lookup needs --atlas-labels with precomputed wireframe".to_string());
            lines.push(format!("region: {region}"));
        }
    } else {
        lines.push("hover over image to inspect voxels | press i for keyboard inspection".to_owned());
        for (i,layer) in layers.overlays.iter().enumerate() {
            lines.push(format!("layer {}: {} | display {}..{}",i+1,layer.name,
                intensity(layer.window.0),intensity(layer.window.1)));
        }
        if layers.atlas.is_some() { lines.push("atlas: move mouse to inspect region".to_owned()); }
    }
    lines.push(if app.inspect_mode {
        "INSPECT arrows/hjkl move crosshair | PgUp/PgDn slices | i exit | g outlines | q quit".to_owned()
    } else {
        "arrows/hjkl slices | i inspector | g outlines | b atlas | r/R rotate | x/y/z axis".to_owned()
    });
    lines
}

fn draw_footer(out: &mut impl Write, vol: &Volume, layers: &Layers,
    app: &App, data_range: (f32,f32)) -> Result<()> {
    let (cols,rows)=terminal::size()?;
    let lines=footer_lines(vol,app,layers,data_range);
    let start=rows.saturating_sub(lines.len() as u16);
    for (i,line) in lines.into_iter().enumerate() {
        if start+i as u16 >= rows { break; }
        queue!(out, MoveTo(0,start+i as u16), Clear(ClearType::CurrentLine),
            Print(crop(line,cols)))?;
    }
    out.flush()?;
    Ok(())
}

fn draw(out: &mut impl Write, vol: &Volume, layers: &Layers, app: &App,
    data_range: (f32, f32), max_pixels: usize, frames: &mut FrameState) -> Result<render::Geometry> {
    let (cols, rows) = terminal::size()?;
    if cols < 20 || rows < (footer_lines(vol,app,layers,data_range).len()+5) as u16 {
        frames.clear(out)?;
        queue!(out, MoveTo(0, 0), Clear(ClearType::All),
            Print("Resize terminal to show all layer values"))?;
        out.flush()?;
        return Ok(render::Geometry {cols:0,rows:0,width:1,height:1});
    }
    let reserved=footer_lines(vol,app,layers,data_range).len()+2;
    let geometry = render::geometry(vol, app, cols, rows, max_pixels,reserved);
    let rgb = render::composite(vol, app, layers, geometry.width, geometry.height);
    let frame = kitty::encode_rgb(&rgb)?;

    let dims = vol.dims();
    let stride = if app.stride > 1 { format!(" | stride {}", app.stride) }
        else { String::new() };
    let header = format!("{} | {}x{}x{} | axis {} | slice {}/{} | rot {}{}",
        app.path, dims[0], dims[1], dims[2], app.axis.label(),
        app.index, vol.depth(app.axis) - 1, app.rotation, stride);
    queue!(out, MoveTo(0, 0), Clear(ClearType::CurrentLine),
        Print(crop(header, cols)), MoveTo(0, 1))?;

    let next = frames.allocate();
    kitty::replace(out, next, &frame, geometry, frames.active_id)?;
    frames.active_id = Some(next);
    draw_footer(out,vol,layers,app,data_range)?;
    Ok(geometry)
}

/// Translate a cell-based or optional pixel-based SGR mouse position to
/// the exact source voxel, matching Kitty's c/r placement + letterboxing.
/// The image begins at column 0, terminal row 1 (after the header).
fn mouse_voxel(vol: &Volume, app: &App, geom: render::Geometry,
    mouse: MouseEvent, pixel_mouse: bool) -> Option<(usize,usize)> {
    let (cols,rows)=terminal::size().ok()?;
    let (cw,ch)=render::cell_pixels(cols,rows);
    let (rx,ry)=mouse_raster(geom,mouse,pixel_mouse,cw,ch)?;
    render::voxel_at_raster(vol,app,rx,ry,geom.width,geom.height)
}

fn mouse_raster(geom: render::Geometry,mouse: MouseEvent,pixel_mouse: bool,
    cw: usize,ch: usize) -> Option<(usize,usize)> {
    if geom.cols==0 || geom.rows==0 || cw==0 || ch==0 { return None; }
    let (mx,my)=if pixel_mouse {
        (mouse.column as f64, mouse.row as f64-ch as f64)
    } else {
        ((mouse.column as f64+0.5)*cw as f64,
         (mouse.row as f64-0.5)*ch as f64)
    };
    let box_w=geom.cols as f64*cw as f64;
    let box_h=geom.rows as f64*ch as f64;
    let scale=(box_w/geom.width as f64).min(box_h/geom.height as f64);
    let image_w=geom.width as f64*scale;
    let image_h=geom.height as f64*scale;
    let x=mx-(box_w-image_w)*0.5;
    let y=my-(box_h-image_h)*0.5;
    if x<0. || y<0. || x>=image_w || y>=image_h { return None; }
    Some(((x/scale).floor() as usize,(y/scale).floor() as usize))
}

fn benchmark(vol: &Volume, layers: &Layers, app: &mut App, config: &Config, count: usize) -> Result<()> {
    let geometry = render::geometry(vol, app, 110, 36, config.max_pixels, 5);
    let start = Instant::now();
    let mut rgb_time = Duration::ZERO;
    let mut encode_time = Duration::ZERO;
    let mut compressed = 0;
    for i in 0..count {
        app.index = i * vol.depth(app.axis) / count;
        let t = Instant::now();
        let rgb = render::composite(vol, app, layers, geometry.width, geometry.height);
        rgb_time += t.elapsed();
        let t = Instant::now();
        compressed += kitty::encode_rgb(&rgb)?.bytes;
        encode_time += t.elapsed();
    }
    let elapsed = start.elapsed();
    println!("niv benchmark — {} frames, {}x{}px, axis {} (no graphics transmission)",
        count, geometry.width, geometry.height, app.axis.label());
    println!("volume {:?}, spacing {:?} (header voxel units), RAM voxels ~{:.1} MiB",
        vol.dims(), vol.spacing, vol.data.len() as f64 * 4.0 / 1_048_576.0);
    println!("rasterize: {:.2} ms/frame | zlib+base64: {:.2} ms/frame | total {:.2} ms/frame",
        rgb_time.as_secs_f64()*1000.0/count as f64,
        encode_time.as_secs_f64()*1000.0/count as f64,
        elapsed.as_secs_f64()*1000.0/count as f64);
    println!("compressed bytes: {:.1} KiB/frame average (before base64 overhead)",
        compressed as f64 / count as f64 / 1024.0);
    Ok(())
}

fn run() -> Result<()> {
    let Some(config) = config::parse()? else { return Ok(()); };
    let t = Instant::now();
    let vol = Volume::load(&config.path)?;
    let load_secs = t.elapsed().as_secs_f64();
    let index = config.initial_slice.unwrap_or(vol.depth(config.axis) / 2);
    if index >= vol.depth(config.axis) {
        bail!("--slice {index} is outside axis {} (0..{})", config.axis.label(), vol.depth(config.axis)-1);
    }
    let window = config.window.unwrap_or_else(|| vol.auto_window());
    let data_range = vol.data_range();
    let layers = Layers::load(&config, &vol)?;
    let name = config.path.file_name().map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| config.path.display().to_string());
    let mut app = App::new(config.axis, index, config.step, config.rotation, window, name);
    app.atlas_style=config.atlas_style;
    if let Some(count) = config.benchmark_frames {
        println!("load: {load_secs:.2}s");
        return benchmark(&vol, &layers, &mut app, &config, count);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("niv needs a real terminal for interactive graphics. Use --bench N for a noninteractive test.");
    }

    let _guard = TerminalGuard::enter(config.enhanced_keys,config.mouse_pixels)?;
    let mut out = io::stdout().lock();
    let mut frames = FrameState::new();
    queue!(out, Clear(ClearType::All))?; // Once, on entering alt-screen.
    let mut geometry=draw(&mut out, &vol, &layers, &app, data_range, config.max_pixels, &mut frames)?;
    let frame_interval = Duration::from_secs_f64(1.0 / config.max_fps as f64);
    let mut last_draw = Instant::now();
    let mut pending = false;
    let mut pending_footer=false;
    loop {
        let wait = if pending || pending_footer {
            frame_interval.saturating_sub(last_draw.elapsed())
        } else { Duration::from_millis(150) };
        if event::poll(wait)? {
            // Coalesce keyboard auto-repeat bursts; at most 256 events per frame.
            for _ in 0..256 {
                match event::read()? {
                    Event::Key(key) => match app.key(&vol, key) {
                        Action::Quit => {
                            frames.clear(&mut out)?;
                            out.flush()?;
                            return Ok(());
                        }
                        Action::Redraw => pending = true,
                        Action::None => {}
                    },
                    Event::Resize(_, _) => {
                        pending = true;
                        last_draw = Instant::now() - frame_interval;
                    }
                    Event::Mouse(mouse) => {
                        if !app.inspect_mode && matches!(mouse.kind,
                            MouseEventKind::Moved | MouseEventKind::Drag(_) | MouseEventKind::Down(_)) {
                            let next=mouse_voxel(&vol,&app,geometry,mouse,config.mouse_pixels);
                            if app.probe!=next {
                                app.probe=next;
                                pending_footer=true;
                            }
                        }
                    }
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? { break; }
            }
        }
        if (pending || pending_footer) && last_draw.elapsed() >= frame_interval {
            if pending {
                geometry=draw(&mut out, &vol, &layers, &app, data_range, config.max_pixels, &mut frames)?;
            } else {
                draw_footer(&mut out, &vol, &layers, &app, data_range)?;
            }
            last_draw = Instant::now();
            pending = false;
            pending_footer=false;
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("niv: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod mouse_tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    fn mouse(column:u16,row:u16) -> MouseEvent {
        MouseEvent {kind:MouseEventKind::Moved,column,row,modifiers:KeyModifiers::NONE}
    }
    #[test]
    fn mouse_cells_and_pixels_hit_matching_raster_coordinates() {
        let geom=render::Geometry {cols:10,rows:5,width:80,height:80};
        // 8x16 px cells; picture begins at row 1.
        assert_eq!(mouse_raster(geom,mouse(2,1),false,8,16),Some((20,8)));
        assert_eq!(mouse_raster(geom,mouse(20,24),true,8,16),Some((20,8)));
        assert_eq!(mouse_raster(geom,mouse(10,1),false,8,16),None);
        assert_eq!(mouse_raster(geom,mouse(2,0),false,8,16),None);
    }
}
