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
    event::{self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use volume::Volume;

struct TerminalGuard { enhanced_keys: bool }
impl TerminalGuard {
    fn enter(enhanced_keys: bool) -> Result<Self> {
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
        Ok(Self { enhanced_keys })
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
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

fn draw(out: &mut impl Write, vol: &Volume, layers: &Layers, app: &App,
    data_range: (f32, f32), max_pixels: usize, frames: &mut FrameState) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    if cols < 20 || rows < 8 {
        frames.clear(out)?;
        queue!(out, MoveTo(0, 0), Clear(ClearType::All),
            Print("Resize terminal (min 20x8)"))?;
        out.flush()?;
        return Ok(());
    }
    let geometry = render::geometry(vol, app, cols, rows, max_pixels);
    let rgb = render::composite(vol, app, layers, geometry.width, geometry.height);
    let frame = kitty::encode_rgb(&rgb)?;

    // Only clear text lines. Clearing the whole screen on every keypress
    // would erase the previous Kitty placement and expose a blank frame.
    let dims = vol.dims();
    let stride = if app.stride > 1 {
        format!(" | stride {}", app.stride)
    } else { String::new() };
    let header = format!("{} | {}x{}x{} | axis {} | slice {}/{} | rot {}{}",
        app.path, dims[0], dims[1], dims[2], app.axis.label(),
        app.index, vol.depth(app.axis) - 1, app.rotation, stride);
    queue!(out, MoveTo(0, 0), Clear(ClearType::CurrentLine),
        Print(crop(header, cols)), MoveTo(0, 1))?;

    // Transmit and place NEW pixels while OLD pixels remain visible.
    // Delete old image only after the complete new placement command.
    let next = frames.allocate();
    kitty::replace(out, next, &frame, geometry, frames.active_id)?;
    frames.active_id = Some(next);

    let mut status = format!("data: {:.3}..{:.3} | display: {:.3}..{:.3}",
        data_range.0, data_range.1, app.window.0, app.window.1);
    if let Some(atlas) = &layers.atlas {
        status.push_str(if app.show_atlas { " | atlas:on" } else { " | atlas:off" });
        if app.show_atlas {
            if let Some(region) = layers.center_region(app.axis, app.index) {
                status.push_str(&format!(" | center: {region}"));
            }
        }
        if atlas.wireframe { status.push_str(" [wireframe]"); }
    }
    if !layers.overlays.is_empty() {
        status.push_str(&format!(" | overlays:{}{}", layers.overlays.len(),
            if app.show_overlays { "" } else { " (off)" }));
    }
    let help = "arrows/hjkl slice | Alt 2x Shift 5x Ctrl 10x | xyz axis | r/R rotate | b atlas | o overlays | +/- contrast";
    queue!(out,
        MoveTo(0, rows - 2), Clear(ClearType::CurrentLine), Print(crop(status, cols)),
        MoveTo(0, rows - 1), Clear(ClearType::CurrentLine), Print(crop(help.into(), cols)))?;
    out.flush()?;
    Ok(())
}

fn benchmark(vol: &Volume, layers: &Layers, app: &mut App, config: &Config, count: usize) -> Result<()> {
    let geometry = render::geometry(vol, app, 110, 36, config.max_pixels);
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
    if let Some(count) = config.benchmark_frames {
        println!("load: {load_secs:.2}s");
        return benchmark(&vol, &layers, &mut app, &config, count);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("niv needs a real terminal for interactive graphics. Use --bench N for a noninteractive test.");
    }

    let _guard = TerminalGuard::enter(config.enhanced_keys)?;
    let mut out = io::stdout().lock();
    let mut frames = FrameState::new();
    queue!(out, Clear(ClearType::All))?; // Once, on entering alt-screen.
    draw(&mut out, &vol, &layers, &app, data_range, config.max_pixels, &mut frames)?;
    let frame_interval = Duration::from_secs_f64(1.0 / config.max_fps as f64);
    let mut last_draw = Instant::now();
    let mut pending = false;
    loop {
        let wait = if pending {
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
                    Event::Resize(_, _) => { pending = true; last_draw = Instant::now() - frame_interval; }
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? { break; }
            }
        }
        if pending && last_draw.elapsed() >= frame_interval {
            draw(&mut out, &vol, &layers, &app, data_range, config.max_pixels, &mut frames)?;
            last_draw = Instant::now();
            pending = false;
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("niv: {err:#}");
        std::process::exit(1);
    }
}
