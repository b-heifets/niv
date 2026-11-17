//! niv — direct Kitty graphics NIfTI slice viewer. No libghostty/Rex dependency.
mod app;
mod config;
mod kitty;
mod render;
mod volume;

use std::{
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use app::{Action, App};
use config::Config;
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

fn draw(out: &mut impl Write, vol: &Volume, app: &App, max_pixels: usize, id: u32) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    if cols < 20 || rows < 8 {
        kitty::delete(out, id)?;
        queue!(out, MoveTo(0, 0), Clear(ClearType::All), Print("Resize terminal (min 20x8)"))?;
        out.flush()?;
        return Ok(());
    }
    let start = Instant::now();
    let geometry = render::geometry(vol, app, cols, rows, max_pixels);
    let rgb = render::grayscale(vol, app, geometry.width, geometry.height);
    let frame = kitty::encode_rgb(&rgb)?;
    let process_ms = start.elapsed().as_secs_f64() * 1_000.0;

    kitty::delete(out, id)?;
    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    let dims = vol.dims();
    let header = format!("niv | {} | {}x{}x{} | index axis {} | slice {}/{} | step {}",
        app.path, dims[0], dims[1], dims[2], app.axis.label(),
        app.index, vol.depth(app.axis) - 1, app.step);
    queue!(out, MoveTo(0, 0), Print(crop(header, cols)), MoveTo(0, 1))?;
    kitty::send(out, id, &frame, geometry)?;
    let status = format!("window {:.3}..{:.3} | {}x{}px | {:.1}ms render+encode | {:.0}KB zlib | flips H:{} V:{}",
        app.window.0, app.window.1, geometry.width, geometry.height,
        process_ms, frame.bytes as f64 / 1024.0,
        if app.flip_h { "on" } else { "off" }, if app.flip_v { "on" } else { "off" });
    let help = "arrows/hjkl scroll | Shift 10x Ctrl 25x Alt 50x | [] step | xyz axis | f/v flip | +/- contrast | q quit";
    queue!(out, MoveTo(0, rows - 2), Print(crop(status, cols)),
        MoveTo(0, rows - 1), Print(crop(help.into(), cols)))?;
    out.flush()?;
    Ok(())
}

fn benchmark(vol: &Volume, app: &mut App, config: &Config, count: usize) -> Result<()> {
    let geometry = render::geometry(vol, app, 110, 36, config.max_pixels);
    let start = Instant::now();
    let mut rgb_time = Duration::ZERO;
    let mut encode_time = Duration::ZERO;
    let mut compressed = 0;
    for i in 0..count {
        app.index = i * vol.depth(app.axis) / count;
        let t = Instant::now();
        let rgb = render::grayscale(vol, app, geometry.width, geometry.height);
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
    let name = config.path.file_name().map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| config.path.display().to_string());
    let mut app = App::new(config.axis, index, config.step, window, name);
    if let Some(count) = config.benchmark_frames {
        println!("load: {load_secs:.2}s");
        return benchmark(&vol, &mut app, &config, count);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("niv needs a real terminal for interactive graphics. Use --bench N for a noninteractive test.");
    }

    let _guard = TerminalGuard::enter(config.enhanced_keys)?;
    let mut out = io::stdout().lock();
    let id = 3_000_000u32.wrapping_add(std::process::id());
    draw(&mut out, &vol, &app, config.max_pixels, id)?;
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
                            kitty::delete(&mut out, id)?;
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
            draw(&mut out, &vol, &app, config.max_pixels, id)?;
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
