//! NIfTI slice viewer. Direct Kitty graphics, no libghostty or Rex dependency.
mod app;
mod config;
mod kitty;
mod layers;
mod render;
mod volume;

use anyhow::{bail, Result};
use app::{Action, App, EditField};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags, MouseEvent,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use layers::Layers;
use std::{
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};
use volume::Volume;

struct TerminalGuard {
    enhanced_keys: bool,
    mouse_pixels: bool,
}
impl TerminalGuard {
    fn enter(enhanced_keys: bool, mouse_pixels: bool) -> Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(err) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(err.into());
        }
        if enhanced_keys {
            if let Err(err) = execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            ) {
                let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
                let _ = terminal::disable_raw_mode();
                return Err(err.into());
            }
        }
        if let Err(err) = execute!(io::stdout(), EnableMouseCapture) {
            if enhanced_keys {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
            return Err(err.into());
        }
        if mouse_pixels {
            io::stdout().write_all(b"\x1b[?1016h")?;
            io::stdout().flush()?;
        }
        Ok(Self {
            enhanced_keys,
            mouse_pixels,
        })
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        if self.mouse_pixels {
            let _ = out.write_all(b"\x1b[?1016l");
        }
        let _ = execute!(out, DisableMouseCapture);
        if self.enhanced_keys {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(out, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

struct FrameState {
    next_id: u32,
    active_id: Option<u32>,
}
impl FrameState {
    fn new() -> Self {
        let pid = std::process::id();
        Self {
            next_id: 3_000_000u32.wrapping_add(pid.wrapping_mul(10_000)),
            active_id: None,
        }
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

#[derive(Clone, Copy)]
struct ViewLayout {
    geom: render::Geometry,
    x: u16,
    y: u16,
    footer: u16,
    list_start: usize,
    list_count: usize,
}
fn layout(vol: &Volume, app: &App, layers: &Layers, cols: u16, rows: u16) -> ViewLayout {
    let list_count = if app.fullscreen || !app.show_layer_list {
        0
    } else {
        (layers.overlays.len() + 1)
            .min(6)
            .min((rows as usize).saturating_sub(7))
    };
    let list_start = (app.selected + 1)
        .saturating_sub(list_count)
        .min(layers.overlays.len() + 1 - list_count);
    let head = if app.fullscreen { 0u16 } else { 1 };
    let footer_rows = if app.fullscreen {
        0u16
    } else {
        3 + list_count as u16
    };
    let reserved = (head + footer_rows) as usize;
    let geom = render::geometry(vol, app, cols, rows, reserved);
    let available = rows.saturating_sub(head + footer_rows);
    let x = cols.saturating_sub(geom.cols) / 2;
    let y = head + available.saturating_sub(geom.rows) / 2;
    ViewLayout {
        geom,
        x,
        y,
        footer: rows.saturating_sub(footer_rows),
        list_start,
        list_count,
    }
}

fn crop(s: &str, cols: usize) -> String {
    s.chars().take(cols).collect()
}
fn intensity(v: f32) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    if v != 0. && (v.abs() < 0.0001 || v.abs() >= 1e7) {
        format!("{v:.4e}")
    } else {
        format!("{v:.3}")
    }
}
fn rgb_color(rgb: [u8; 3]) -> Color {
    Color::Rgb {
        r: rgb[0],
        g: rgb[1],
        b: rgb[2],
    }
}

fn line(out: &mut impl Write, row: u16, s: &str, cols: u16, fg: Color) -> Result<()> {
    queue!(
        out,
        MoveTo(0, row),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(fg),
        Print(crop(s, cols as usize)),
        ResetColor
    )?;
    Ok(())
}

fn selected_name<'a>(app: &'a App, layers: &'a Layers) -> &'a str {
    if app.selected == 0 {
        &app.path
    } else {
        &layers.overlays[app.selected - 1].name
    }
}
fn selected_data_range(app: &App, layers: &Layers, base_range: (f32, f32)) -> (f32, f32) {
    if app.selected == 0 {
        base_range
    } else {
        layers.overlays[app.selected - 1].data_range
    }
}
fn selected_visible(app: &App, layers: &Layers) -> bool {
    app.selected == 0 || (app.show_overlays && layers.overlays[app.selected - 1].visible)
}

fn draw_ui(
    out: &mut impl Write,
    vol: &Volume,
    layers: &Layers,
    app: &App,
    base_range: (f32, f32),
    view: ViewLayout,
) -> Result<()> {
    if app.fullscreen {
        return Ok(());
    }
    let (cols, rows) = terminal::size()?;
    // First row: bold region label and LUT color, followed by dim atlas file name.
    queue!(out, MoveTo(0, 0), Clear(ClearType::CurrentLine))?;
    let mut consumed = 0usize;
    if let Some((h, v)) = app.probe {
        if let Some((label, color)) = layers.region_details(app.axis, app.index, h, v) {
            let prefix = "Region: ";
            queue!(out, SetAttribute(Attribute::Bold), Print(prefix))?;
            consumed += prefix.chars().count();
            let remaining = (cols as usize).saturating_sub(consumed);
            let label = crop(&label, remaining);
            if let Some(color) = color {
                queue!(out, SetForegroundColor(rgb_color(color)))?;
            }
            queue!(
                out,
                Print(&label),
                ResetColor,
                SetAttribute(Attribute::Reset)
            )?;
            consumed += label.chars().count();
        }
    }
    if let Some(atlas) = &app.atlas_name {
        let text = if consumed > 0 {
            format!("   |   {atlas}")
        } else {
            format!("Atlas: {atlas}")
        };
        if consumed < cols as usize {
            queue!(
                out,
                SetForegroundColor(Color::DarkGrey),
                Print(crop(&text, (cols as usize) - consumed)),
                ResetColor
            )?;
        }
    } else if consumed == 0 {
        let header = format!(
            "Slice {}/{}    stride {}",
            app.index + 1,
            vol.depth(app.axis),
            app.stride
        );
        queue!(
            out,
            SetForegroundColor(Color::DarkGrey),
            Print(crop(&header, cols as usize)),
            ResetColor
        )?;
    }
    let win = app.selected_window(layers);
    let dat = selected_data_range(app, layers, base_range);
    let palette = app.selected_palette(layers);
    let status = if selected_visible(app, layers) {
        ""
    } else {
        " [hidden]"
    };
    let layer_name = if app.show_layer_list {
        String::new()
    } else {
        format!("  {}", selected_name(app, layers))
    };
    let summary = format!(
        "{}{}{}  |  {}  |  min {}   max {}  |  data {}..{}",
        app.selected + 1,
        layer_name,
        status,
        palette.name(),
        intensity(win.0),
        intensity(win.1),
        intensity(dat.0),
        intensity(dat.1)
    );
    queue!(
        out,
        MoveTo(0, view.footer),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(rgb_color(palette.accent(app.selected))),
        SetAttribute(Attribute::Bold),
        Print(crop(&summary, cols as usize)),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    // Optional compact list: every filename is shown at most once.
    for k in 0..view.list_count {
        let idx = view.list_start + k;
        let name = if idx == 0 {
            &app.path
        } else {
            &layers.overlays[idx - 1].name
        };
        let pal = if idx == 0 {
            app.base_palette
        } else {
            layers.overlays[idx - 1].palette
        };
        let is_visible = idx == 0 || (app.show_overlays && layers.overlays[idx - 1].visible);
        let mark = if idx == app.selected { ">" } else { " " };
        let row = view.footer + 1 + k as u16;
        let txt = format!(
            "{} {}  {}  {}{}",
            mark,
            idx + 1,
            name,
            pal.name(),
            if is_visible { "" } else { " [hidden]" }
        );
        line(
            out,
            row,
            &txt,
            cols,
            if idx == app.selected {
                rgb_color(pal.accent(idx))
            } else {
                Color::DarkGrey
            },
        )?;
    }
    let inspect_row = view.footer + 1 + view.list_count as u16;
    let info = if let Some((h, v)) = app.probe {
        let [x, y, z] = app.axis.xyz(app.index, h, v);
        let mut text = format!(
            "[{x},{y},{z}]   1: {}",
            intensity(vol.sample(app.axis, app.index, h, v))
        );
        for (i, layer) in layers.overlays.iter().enumerate() {
            text.push_str(&format!(
                "   {}: {}",
                i + 2,
                intensity(layer.vol.sample(app.axis, app.index, h, v))
            ));
        }
        if app.stride > 1 {
            text.push_str(&format!("   stride {}", app.stride));
        }
        text
    } else {
        format!(
            "Slice {}/{}   stride {}   |   hover to inspect voxels, or press i",
            app.index + 1,
            vol.depth(app.axis),
            app.stride
        )
    };
    line(out, inspect_row, &info, cols, Color::Grey)?;
    let help_row = rows - 1;
    if let Some(edit) = &app.edit {
        let label = match edit.field {
            EditField::Both => "min,max",
            EditField::Minimum => "minimum",
            EditField::Maximum => "maximum",
        };
        let error = edit.error.as_deref().unwrap_or("");
        let prompt = format!(
            "{label} > {}_   {error}    Enter apply / Esc cancel",
            edit.text
        );
        line(
            out,
            help_row,
            &prompt,
            cols,
            if edit.error.is_some() {
                Color::Red
            } else {
                Color::Yellow
            },
        )?;
    } else {
        let help = if app.inspect_mode {
            "INSPECT: arrows move probe  PgUp/PgDn slice  i exit  Tab image-only  a atlas"
        } else {
            "wheel/arrows slices  [/ ] stride  1-9 layer  e range  c color  a atlas  t list  Tab fill"
        };
        line(out, help_row, help, cols, Color::DarkGrey)?;
    }
    out.flush()?;
    Ok(())
}

fn draw(
    out: &mut impl Write,
    vol: &Volume,
    layers: &Layers,
    app: &App,
    base_range: (f32, f32),
    frames: &mut FrameState,
) -> Result<ViewLayout> {
    let (cols, rows) = terminal::size()?;
    if cols < 16 || rows < 6 {
        frames.clear(out)?;
        queue!(
            out,
            MoveTo(0, 0),
            Clear(ClearType::All),
            Print("Enlarge terminal")
        )?;
        out.flush()?;
        return Ok(ViewLayout {
            geom: render::Geometry {
                cols: 0,
                rows: 0,
                width: 1,
                height: 1,
            },
            x: 0,
            y: 0,
            footer: 0,
            list_start: 0,
            list_count: 0,
        });
    }
    let view = layout(vol, app, layers, cols, rows);
    let rgb = render::composite(vol, app, layers, view.geom.width, view.geom.height);
    let frame = kitty::encode_rgb(&rgb)?;
    queue!(out, MoveTo(view.x, view.y))?;
    let next = frames.allocate();
    kitty::replace(out, next, &frame, view.geom, frames.active_id)?;
    frames.active_id = Some(next);
    draw_ui(out, vol, layers, app, base_range, view)?;
    out.flush()?;
    Ok(view)
}

fn mouse_voxel(
    vol: &Volume,
    app: &App,
    view: ViewLayout,
    mouse: MouseEvent,
    pixel_mouse: bool,
) -> Option<(usize, usize)> {
    let (cols, rows) = terminal::size().ok()?;
    let (cw, ch) = render::cell_pixels(cols, rows);
    let (rx, ry) = mouse_raster(view, mouse, pixel_mouse, cw, ch)?;
    render::voxel_at_raster(vol, app, rx, ry, view.geom.width, view.geom.height)
}

fn mouse_raster(
    view: ViewLayout,
    mouse: MouseEvent,
    pixel_mouse: bool,
    cw: usize,
    ch: usize,
) -> Option<(usize, usize)> {
    let geom = view.geom;
    if geom.cols == 0 || geom.rows == 0 || cw == 0 || ch == 0 {
        return None;
    }
    let (mx, my) = if pixel_mouse {
        (
            mouse.column as f64 - view.x as f64 * cw as f64,
            mouse.row as f64 - view.y as f64 * ch as f64,
        )
    } else {
        (
            (mouse.column as f64 + 0.5 - view.x as f64) * cw as f64,
            (mouse.row as f64 + 0.5 - view.y as f64) * ch as f64,
        )
    };
    let box_w = geom.cols as f64 * cw as f64;
    let box_h = geom.rows as f64 * ch as f64;
    let scale = (box_w / geom.width as f64).min(box_h / geom.height as f64);
    let display_w = geom.width as f64 * scale;
    let display_h = geom.height as f64 * scale;
    let x = mx - (box_w - display_w) * 0.5;
    let y = my - (box_h - display_h) * 0.5;
    if x < 0. || y < 0. || x >= display_w || y >= display_h {
        return None;
    }
    Some(((x / scale).floor() as usize, (y / scale).floor() as usize))
}

fn benchmark(vol: &Volume, layers: &Layers, app: &mut App, count: usize) -> Result<()> {
    let geom = render::geometry(vol, app, 110, 36, 4);
    let start = Instant::now();
    let mut rgb_time = Duration::ZERO;
    let mut encode_time = Duration::ZERO;
    let mut compressed = 0;
    for i in 0..count {
        app.index = i * vol.depth(app.axis) / count;
        let t = Instant::now();
        let rgb = render::composite(vol, app, layers, geom.width, geom.height);
        rgb_time += t.elapsed();
        let t = Instant::now();
        compressed += kitty::encode_rgb(&rgb)?.bytes;
        encode_time += t.elapsed();
    }
    let elapsed = start.elapsed();
    println!(
        "{} frames, {}x{}px, axis {} (no graphics transmission)",
        count,
        geom.width,
        geom.height,
        app.axis.label()
    );
    println!(
        "volume {:?}, spacing {:?}, RAM voxels ~{:.1} MiB",
        vol.dims(),
        vol.spacing,
        vol.data.len() as f64 * 4. / 1_048_576.
    );
    println!(
        "rasterize {:.2}ms/frame | encode {:.2}ms/frame | total {:.2}ms/frame",
        rgb_time.as_secs_f64() * 1000. / count as f64,
        encode_time.as_secs_f64() * 1000. / count as f64,
        elapsed.as_secs_f64() * 1000. / count as f64
    );
    println!(
        "compressed {:.1} KiB/frame before base64",
        compressed as f64 / count as f64 / 1024.
    );
    Ok(())
}

fn run() -> Result<()> {
    let Some(config) = config::parse()? else {
        return Ok(());
    };
    let vol = Volume::load(&config.path)?;
    let index = config.initial_slice.unwrap_or(vol.depth(config.axis) / 2);
    if index >= vol.depth(config.axis) {
        bail!(
            "--slice {index} is outside axis {} (0..{})",
            config.axis.label(),
            vol.depth(config.axis) - 1
        );
    }
    let window = config.window.unwrap_or_else(|| vol.auto_window());
    let base_range = vol.data_range();
    let mut layers = Layers::load(&config, &vol)?;
    let name = layers::short_name(&config.path);
    let mut app = App::new(
        config.axis,
        index,
        config.step,
        config.rotation,
        window,
        name,
    );
    app.atlas_name = config.atlas_path.as_ref().map(|p| layers::short_name(p));
    if let Some(count) = config.benchmark_frames {
        return benchmark(&vol, &layers, &mut app, count);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("Interactive graphics require a terminal; use --bench N for a headless test");
    }
    let _guard = TerminalGuard::enter(config.enhanced_keys, config.mouse_pixels)?;
    let mut out = io::stdout().lock();
    let mut frames = FrameState::new();
    queue!(out, Clear(ClearType::All))?;
    let mut view = draw(&mut out, &vol, &layers, &app, base_range, &mut frames)?;
    let frame_interval = Duration::from_secs_f64(1.0 / config.max_fps as f64);
    let mut last_draw = Instant::now();
    let mut pending_frame = false;
    let mut pending_ui = false;
    let mut pending_layout = false;
    loop {
        let wait = if pending_frame || pending_ui || pending_layout {
            frame_interval.saturating_sub(last_draw.elapsed())
        } else {
            Duration::from_millis(150)
        };
        if event::poll(wait)? {
            for _ in 0..256 {
                match event::read()? {
                    Event::Key(key) => match app.key(&vol, &mut layers, key) {
                        Action::Quit => {
                            frames.clear(&mut out)?;
                            out.flush()?;
                            return Ok(());
                        }
                        Action::Redraw => pending_frame = true,
                        Action::Ui => pending_ui = true,
                        Action::Layout => {
                            pending_layout = true;
                            pending_frame = true;
                        }
                        Action::None => {}
                    },
                    Event::Resize(_, _) => {
                        pending_layout = true;
                        pending_frame = true;
                        last_draw = Instant::now() - frame_interval;
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            if app.edit.is_none() =>
                        {
                            let direction = if mouse.kind == MouseEventKind::ScrollUp {
                                1
                            } else {
                                -1
                            };
                            let multiplier = App::multiplier(mouse.modifiers, false);
                            if app.move_slices(&vol, direction * (app.stride * multiplier) as isize)
                            {
                                pending_frame = true;
                            }
                        }
                        MouseEventKind::Moved
                        | MouseEventKind::Drag(_)
                        | MouseEventKind::Down(_)
                            if !app.inspect_mode && app.edit.is_none() =>
                        {
                            let next = mouse_voxel(&vol, &app, view, mouse, config.mouse_pixels);
                            if app.probe != next {
                                app.probe = next;
                                pending_ui = true;
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        if (pending_frame || pending_ui || pending_layout) && last_draw.elapsed() >= frame_interval
        {
            if pending_layout {
                frames.clear(&mut out)?;
                queue!(out, Clear(ClearType::All))?;
            }
            if pending_frame {
                view = draw(&mut out, &vol, &layers, &app, base_range, &mut frames)?;
            } else if pending_ui {
                let (cols, rows) = terminal::size()?;
                view = layout(&vol, &app, &layers, cols, rows);
                draw_ui(&mut out, &vol, &layers, &app, base_range, view)?;
            }
            pending_frame = false;
            pending_ui = false;
            pending_layout = false;
            last_draw = Instant::now();
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
    fn mouse(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }
    #[test]
    fn mouse_uses_placement_offsets() {
        let view = ViewLayout {
            geom: render::Geometry {
                cols: 10,
                rows: 5,
                width: 80,
                height: 80,
            },
            x: 2,
            y: 3,
            footer: 15,
            list_start: 0,
            list_count: 0,
        };
        assert_eq!(
            mouse_raster(view, mouse(4, 4), false, 8, 16),
            Some((20, 24))
        );
        assert_eq!(
            mouse_raster(view, mouse(20 + 16, 24 + 48), true, 8, 16),
            Some((20, 24))
        );
        assert_eq!(mouse_raster(view, mouse(1, 3), false, 8, 16), None);
    }
}
