//! NIfTI slice viewer. Direct Kitty graphics, no libghostty or Rex dependency.
mod app;
mod config;
mod kitty;
mod layers;
mod montage;
mod path_complete;
mod projection;
mod render;
mod scene;
mod volume;

use anyhow::{bail, Result};
use app::{Action, App, EditField, Popup};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags, MouseEvent,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
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
                        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
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

#[derive(Default)]
struct FrameStats {
    frames: usize,
    raster: Duration,
    encode: Duration,
    output: Duration,
    compressed_bytes: usize,
}
impl FrameStats {
    fn report(&self) -> String {
        if self.frames == 0 {
            return "No frames rendered".into();
        }
        let n = self.frames as f64;
        format!("niv profile: {} frames | raster {:.2} ms | encode {:.2} ms | terminal write {:.2} ms | compressed {:.1} KiB/frame\nNote: terminal write excludes asynchronous Rex/Ghostty rendering and network delivery.",
            self.frames,self.raster.as_secs_f64()*1000.0/n,
            self.encode.as_secs_f64()*1000.0/n,self.output.as_secs_f64()*1000.0/n,
            self.compressed_bytes as f64/n/1024.0)
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
    let extra = usize::from(app.edit.is_some() || app.notice.is_some());
    let list_count = if app.fullscreen || !app.show_layer_list {
        0
    } else {
        (layers.overlays.len() + 1)
            .min(6)
            .min((rows as usize).saturating_sub(8 + extra))
    };
    let list_start = (app.selected + 1)
        .saturating_sub(list_count)
        .min(layers.overlays.len() + 1 - list_count);
    // Region and probe always sit ABOVE the image. Only the selected layer,
    // optional list, and active editor/status message sit below it.
    let head = if app.fullscreen { 0u16 } else { 2 };
    let footer_rows = if app.fullscreen {
        0u16
    } else {
        1 + list_count as u16 + extra as u16
    };
    let reserved = (head + footer_rows) as usize;
    let geom = if app.montage.enabled {
        montage::plan(vol, app, cols, rows, reserved).geom
    } else {
        render::geometry(vol, app, cols, rows, reserved)
    };
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

// A change in reserved UI rows or image placement must clear the old text
// even when the originating action was a simple raster redraw. The list's
// scroll offset is purely textual and does not invalidate the image.
fn layout_changed(old: ViewLayout, new: ViewLayout) -> bool {
    old.x != new.x
        || old.y != new.y
        || old.footer != new.footer
        || old.list_count != new.list_count
        || old.geom.cols != new.geom.cols
        || old.geom.rows != new.geom.rows
        || old.geom.width != new.geom.width
        || old.geom.height != new.geom.height
}

/// Map a visible list row to its bottom-based stack index. The image at
/// the top of the stack must also appear at the top of the layer list.
fn top_first_position(start: usize, count: usize, row: usize) -> usize {
    debug_assert!(row < count);
    start + count - 1 - row
}

fn crop(s: &str, cols: usize) -> String {
    s.chars().take(cols).collect()
}
fn middle_ellipsis(s: &str, limit: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= limit {
        return s.to_owned();
    }
    if limit < 5 {
        return crop(s, limit);
    }
    let head = (limit - 1) / 2;
    let tail = limit - 1 - head;
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}

fn intensity(v: f32) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    format!("{v:.7}")
}
fn compact_number(v: f32) -> String {
    if v.is_finite() && v.abs() < 100_000.0 {
        format!("{v:.5}")
    } else {
        format!("{v:.5e}")
    }
}
fn rgb_color(rgb: [u8; 3]) -> Color {
    Color::Rgb {
        r: rgb[0],
        g: rgb[1],
        b: rgb[2],
    }
}

fn line(out: &mut impl Write, row: u16, s: &str, cols: u16, fg: Color, x: u16) -> Result<()> {
    queue!(
        out,
        MoveTo(0, row),
        Clear(ClearType::CurrentLine),
        MoveTo(x, row),
        SetForegroundColor(fg),
        Print(crop(s, cols.saturating_sub(x) as usize)),
        ResetColor
    )?;
    Ok(())
}

// A deterministic, smooth-looking rainbow for categorical filenames only.
// Limit the number of terminal color escapes even for very long filenames.
fn rainbow_runs(s: &str) -> Vec<(Color, String)> {
    const STOPS: [[u8; 3]; 7] = [
        [255, 115, 132],
        [255, 179, 92],
        [237, 221, 100],
        [126, 220, 139],
        [96, 213, 224],
        [132, 168, 255],
        [232, 129, 255],
    ];
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let n = chars.len().min(12);
    (0..n)
        .map(|i| {
            let start = i * chars.len() / n;
            let end = (i + 1) * chars.len() / n;
            let t = i as f32 * 6.0 / n.saturating_sub(1).max(1) as f32;
            let k = (t.floor() as usize).min(5);
            let f = t - k as f32;
            let rgb = std::array::from_fn(|c| {
                (STOPS[k][c] as f32 * (1.0 - f) + STOPS[k + 1][c] as f32 * f).round() as u8
            });
            (rgb_color(rgb), chars[start..end].iter().collect())
        })
        .collect()
}

fn rainbow_layer_line(
    out: &mut impl Write,
    row: u16,
    prefix: &str,
    name: &str,
    suffix: &str,
    cols: u16,
    x: u16,
    fg: Color,
) -> Result<()> {
    let available = cols.saturating_sub(x) as usize;
    let prefix = crop(prefix, available);
    let name = crop(name, available.saturating_sub(prefix.chars().count()));
    let suffix = crop(
        suffix,
        available.saturating_sub(prefix.chars().count() + name.chars().count()),
    );
    queue!(
        out,
        MoveTo(0, row),
        Clear(ClearType::CurrentLine),
        MoveTo(x, row),
        SetForegroundColor(fg),
        Print(prefix)
    )?;
    for (color, run) in rainbow_runs(&name) {
        queue!(out, SetForegroundColor(color), Print(run))?;
    }
    queue!(out, SetForegroundColor(fg), Print(suffix), ResetColor)?;
    Ok(())
}

fn selected_name<'a>(app: &'a App, layers: &'a Layers) -> &'a str {
    let id = app.selected_layer_id();
    if id == 0 {
        &app.path
    } else {
        &layers.overlays[id - 1].name
    }
}

fn selected_visible(app: &App, layers: &Layers) -> bool {
    let id = app.selected_layer_id();
    app.layer_visible(layers, id)
}

fn draw_ui(
    out: &mut impl Write,
    vol: &Volume,
    layers: &Layers,
    app: &App,
    _base_range: (f32, f32),
    view: ViewLayout,
) -> Result<()> {
    if app.fullscreen {
        return Ok(());
    }
    let (cols, rows) = terminal::size()?;
    let x = view.x;
    let footer_x = 0u16;
    // Row 1: the region itself and a tiny help indicator at the right edge.
    queue!(out, MoveTo(0, 0), Clear(ClearType::CurrentLine))?;
    if layers.atlas.is_some() && app.projection == app::ProjectionMode::Slice {
        let (w, h, _) = vol.plane(app.axis);
        let (vh, vv) = app.probe.unwrap_or((w / 2, h / 2));
        if let Some((label, color)) = layers.region_details(app.axis, app.index, vh, vv) {
            queue!(out, MoveTo(x, 0), SetAttribute(Attribute::Bold))?;
            if let Some(rgb) = color {
                queue!(out, SetForegroundColor(rgb_color(rgb)))?;
            }
            queue!(
                out,
                Print(middle_ellipsis(
                    &label,
                    cols.saturating_sub(x + 19) as usize
                )),
                ResetColor,
                SetAttribute(Attribute::Reset)
            )?;
        }
    } else if app.projection != app::ProjectionMode::Slice {
        let outline = match &layers.atlas {
            Some(atlas)
                if app.show_atlas
                    && atlas.opacity > 0.
                    && (!atlas.wireframe || atlas.labels.is_some()) =>
            {
                " · atlas silhouette"
            }
            Some(atlas) if app.show_atlas && atlas.wireframe && atlas.labels.is_none() => {
                " · solid atlas required for silhouette"
            }
            _ => "",
        };
        let label = format!(
            "{} along {}{outline}",
            app.projection.label(),
            app.axis.label()
        );
        queue!(
            out,
            MoveTo(x, 0),
            SetAttribute(Attribute::Bold),
            Print(middle_ellipsis(
                &label,
                cols.saturating_sub(x + 19) as usize
            )),
            SetAttribute(Attribute::Reset)
        )?;
    }
    let hint = "Press ? for help";
    if cols as usize >= hint.len() + 2 {
        queue!(
            out,
            MoveTo(cols - hint.len() as u16 - 1, 0),
            SetForegroundColor(Color::DarkGrey),
            Print(hint),
            ResetColor
        )?;
    }
    // Row 2: permanent readout of actual voxel values, even without hover.
    let (w, h, _) = vol.plane(app.axis);
    let (vh, vv) = app.probe.unwrap_or((w / 2, h / 2));
    let [ix, iy, iz] = app.axis.xyz(app.index, vh, vv);
    let mut inspect = if app.projection == app::ProjectionMode::Slice {
        format!("[{ix},{iy},{iz}]")
    } else {
        match app.axis {
            volume::Axis::X => format!("[*,{iy},{iz}]"),
            volume::Axis::Y => format!("[{ix},*,{iz}]"),
            volume::Axis::Z => format!("[{ix},{iy},*]"),
        }
    };
    // Number images by the same bottom-to-top order as the layer selector.
    for pos in 0..=layers.overlays.len() {
        let id = app.layer_at(pos);
        let value = if id == 0 {
            render::display_value(vol, app, id, vh, vv)
        } else {
            render::display_value(&layers.overlays[id - 1].vol, app, id, vh, vv)
        };
        inspect.push_str(&format!("  |  {}: {}", pos + 1, intensity(value)));
    }
    line(out, 1, &inspect, cols, Color::Grey, x)?;

    // Layer window appears BELOW the image to separate it from voxel/atlas text.
    let (lo, hi) = app.selected_window(layers);
    let opacity = app.selected_opacity(layers);
    let accent = app.selected_palette(layers).accent(app.selected_layer_id());
    let marker = if selected_visible(app, layers) {
        ""
    } else {
        " [off]"
    };
    let settings = format!(
        "  |  min: {}  |  max: {}  |  α: {:.0}%{}",
        compact_number(lo),
        compact_number(hi),
        opacity * 100.0,
        marker
    );
    let prefix = format!("{}: ", app.selected + 1);
    let name_space = (cols as usize)
        .saturating_sub(footer_x as usize + prefix.len() + settings.chars().count() + 1);
    let name = middle_ellipsis(selected_name(app, layers), name_space);
    let summary = if app.montage.enabled {
        let reserved = if app.fullscreen {
            0
        } else {
            2 + 1 + view.list_count + usize::from(app.edit.is_some() || app.notice.is_some())
        };
        let plan = montage::plan(vol, app, cols, rows, reserved);
        format!(
            "Montage {}–{} / {}  |  {}×{}  |  gap {} px  |  {}/{} shown  |  row {}/{}  |  slice {}",
            app.montage.start,
            app.montage.stop,
            if app.montage.interval == 0 {
                "auto".into()
            } else {
                app.montage.interval.to_string()
            },
            plan.rows,
            plan.columns,
            plan.gap,
            plan.tiles.len(),
            plan.total_slices,
            plan.page + 1,
            plan.pages,
            app.index
        )
    } else {
        format!("{prefix}{name}{settings}")
    };
    if !app.montage.enabled
        && selected_visible(app, layers)
        && app.selected_palette(layers) == layers::Palette::Random
    {
        rainbow_layer_line(
            out,
            view.footer,
            &prefix,
            &name,
            &settings,
            cols,
            footer_x,
            Color::Grey,
        )?;
    } else {
        line(
            out,
            view.footer,
            &summary,
            cols,
            if app.montage.enabled {
                Color::Grey
            } else if !selected_visible(app, layers) {
                Color::DarkGrey
            } else {
                rgb_color(accent)
            },
            footer_x,
        )?;
    }

    for k in 0..view.list_count {
        let idx = top_first_position(view.list_start, view.list_count, k);
        let id = app.layer_at(idx);
        let name = if id == 0 {
            &app.path
        } else {
            &layers.overlays[id - 1].name
        };
        let pal = if id == 0 {
            app.base_palette
        } else {
            layers.overlays[id - 1].palette
        };
        let visible = app.layer_visible(layers, id);
        let mark = if idx == app.selected { ">" } else { " " };
        let prefix = format!("{} {}: ", mark, idx + 1);
        let max_name = (cols as usize).saturating_sub(footer_x as usize + prefix.len() + 7);
        let shown = middle_ellipsis(name, max_name);
        if visible && pal == layers::Palette::Random {
            rainbow_layer_line(
                out,
                view.footer + 1 + k as u16,
                &prefix,
                &shown,
                "",
                cols,
                footer_x,
                Color::Grey,
            )?;
        } else {
            let txt = format!("{}{}{}", prefix, shown, if visible { "" } else { " [off]" });
            line(
                out,
                view.footer + 1 + k as u16,
                &txt,
                cols,
                if !visible {
                    Color::DarkGrey
                } else if idx == app.selected {
                    rgb_color(pal.accent(id))
                } else {
                    Color::Grey
                },
                footer_x,
            )?;
        }
    }
    // No persistent bottom help row. Only transient prompts/errors/loading.
    if app.edit.is_some() || app.notice.is_some() {
        let bottom = rows - 1;
        if let Some(edit) = &app.edit {
            let what = match edit.field {
                EditField::Both => "min,max",
                EditField::Minimum => "minimum",
                EditField::Maximum => "maximum",
                EditField::LayerOpacity => "image opacity (0..1)",
                EditField::AtlasOpacity => "atlas opacity (0..1)",
                EditField::AddPath => "new overlay path",
                EditField::SaveScene => "save scene as",
                EditField::MontageStart => "start slice (0-based)",
                EditField::MontageStop => "stop slice (0-based)",
                EditField::MontageColumns => "columns (zoom)",
                EditField::MontageRows => "rows",
                EditField::MontageGap => "tile gap (px)",
                EditField::MontageInterval => "slice interval (0 = auto)",
                EditField::AtlasBlur => "atlas blur sigma (px, 0 = off)",
            };
            let err = edit.error.as_deref().unwrap_or("");
            let hint = if let Some(v) = app.numeric_montage_value(edit.field) {
                format!("current: {v} · ↑↓ adjust")
            } else {
                // Display the shortest round-trippable f32, not the five-decimal footer.
                match edit.field {
                    EditField::Minimum => format!("current: {lo}"),
                    EditField::Maximum => format!("current: {hi}"),
                    EditField::Both => format!("current: {lo},{hi}"),
                    _ => edit
                        .completion
                        .as_ref()
                        .map(|m| m.status())
                        .unwrap_or_default(),
                }
            };
            let cols_available = cols.saturating_sub(footer_x) as usize;
            let hint = middle_ellipsis(&hint, (cols_available / 3).max(12));
            let overhead = what.chars().count() + err.chars().count() + hint.chars().count() + 24;
            let shown = middle_ellipsis(&edit.text, cols_available.saturating_sub(overhead).max(8));
            let txt = format!("{what} > {shown}_  {err}  {hint}    Enter apply · Esc cancel");
            line(
                out,
                bottom,
                &txt,
                cols,
                if edit.error.is_some() {
                    Color::Red
                } else {
                    Color::Yellow
                },
                footer_x,
            )?;
        } else if let Some(notice) = &app.notice {
            line(out, bottom, notice, cols, Color::Yellow, footer_x)?;
        }
    }
    out.flush()?;
    Ok(())
}

/// Visible file picker, rather than blind cycling of one filename at a time.
/// This hides the Kitty placement while the picker is open, so selections only
/// redraw terminal text (not an expensive NIfTI raster).
fn draw_picker(out: &mut impl Write, app: &App) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    queue!(out, Clear(ClearType::All))?;
    let Some(editor) = &app.edit else {
        return Ok(());
    };
    let Some(matches) = &editor.completion else {
        return Ok(());
    };
    let x = if cols >= 70 { 3 } else { 1 };
    let width = cols.saturating_sub(x + 1) as usize;
    if rows == 0 {
        return Ok(());
    }
    line(out, 0, "Load NIfTI images", cols, Color::White, x)?;
    if rows > 1 {
        let filter = if editor.text.is_empty() {
            "(type any substring)"
        } else {
            &editor.text
        };
        line(out, 1, &format!("Filter: {filter}"), cols, Color::Yellow, x)?;
    }
    if rows > 2 {
        let directory = if matches.directory.is_empty() {
            "."
        } else {
            &matches.directory
        };
        line(
            out,
            2,
            &format!(
                "{} · marked {} · in {}",
                matches.status(),
                editor.checked.len(),
                directory
            ),
            cols,
            Color::DarkGrey,
            x,
        )?;
    }
    let visible = (rows as usize).saturating_sub(8).min(18);
    let selected = matches.selected_index();
    let start = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(matches.values.len().saturating_sub(visible));
    for (line_idx, item) in matches.values.iter().enumerate().skip(start).take(visible) {
        let row = 3 + (line_idx - start) as u16;
        let active = line_idx == selected;
        let marked = editor
            .checked
            .iter()
            .any(|p| p == &crate::path_complete::resolve(&item.path));
        let prefix = format!(
            "{} {} ",
            if active { ">" } else { " " },
            if marked { "[x]" } else { "[ ]" }
        );
        let label = item
            .path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(&item.path);
        let display = format!(
            "{prefix}{}{}",
            middle_ellipsis(label, width.saturating_sub(9)),
            if item.is_dir { " /" } else { "" }
        );
        line(
            out,
            row,
            &display,
            cols,
            if active { Color::Cyan } else { Color::Grey },
            x,
        )?;
    }
    if rows >= 5 {
        let row = rows - 4;
        let selected_path = matches.selected().map(|m| m.path.as_str()).unwrap_or("");
        line(
            out,
            row,
            &format!(
                "Selected: {}",
                middle_ellipsis(selected_path, width.saturating_sub(11))
            ),
            cols,
            Color::Grey,
            x,
        )?;
    }
    if rows >= 4 {
        if let Some(notice) = &app.notice {
            line(out, rows - 3, notice, cols, Color::Yellow, x)?;
        } else if let Some(e) = &editor.error {
            line(out, rows - 3, &format!("Error: {e}"), cols, Color::Red, x)?;
        } else {
            line(
                out,
                rows - 3,
                "↑↓ choose · Tab complete/cycle · → browse directory · Space mark",
                cols,
                Color::DarkGrey,
                x,
            )?;
        }
        if app.notice.is_none() {
            line(
                out,
                rows - 2,
                "Enter load selected/marked · Esc cancel · PgUp/PgDn page",
                cols,
                Color::DarkGrey,
                x,
            )?;
        }
    }
    out.flush()?;
    Ok(())
}

/// Small modal sheet; the graphics placement is temporarily hidden while open
/// to ensure text remains readable even in multiplexers that draw Kitty images
/// above terminal character cells.
fn draw_popup(
    out: &mut impl Write,
    vol: &Volume,
    layers: &Layers,
    app: &App,
    base_range: (f32, f32),
) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    queue!(out, Clear(ClearType::All))?;
    let mut lines: Vec<String> = Vec::new();
    match app.popup {
        Some(Popup::Help) => {
            lines.extend(
                [
                    "NAVIGATION",
                    "  ↑↓←→ / hjkl      Slices; wheel / trackpad also scroll",
                    "  Cmd ×2 · Shift ×3 · Ctrl ×4 · Alt ×2 fallback",
                    "  PgUp / PgDn      ±25 slices    x y z   choose axis",
                    "  [ / ] stride   r / R rotate   f / v flip   Tab fill",
                    "  i      Keyboard probe (arrows move probe; PgUp/Dn slices)",
                    "",
                    "  p      Montage mode (press ? there for controls)",
                    "",
                    "LAYERS",
                    "  1–9    Select layer      ;       Next layer",
                    "  n      File picker: filter filename by substring",
                    "  s      Save current scene (.niv.toml)",
                    "         ↑↓ select · Tab complete · Space mark · Enter load",
                    "  { / }  Lower / raise selected image (Shift+[ / ])",
                    "  Reload saved scene: niv --scene FILE.niv.toml",
                    "         Atlas always stays on top; t shows stack order",
                    "  Space  Toggle selected image",
                    "  e      Edit min,max     m / M   Edit just min / max",
                    "  0      Reset range      c       Cycle color map",
                    "  o      Image opacity    O       Atlas opacity",
                    "  a      Atlas visible    A       Average projection on/off",
                    "  g      Edge AA (optional)    X   Max projection on/off",
                    "  Projections: current axis, full depth; atlas silhouette only",
                    "  G      Atlas Gaussian blur (0 = off)",
                    "  b      Black / transparent",
                    "  t      Layer list: ↑↓ / jk select; Enter / q / Esc close",
                    "  + / -  Contrast         , / .   Brightness",
                    "  In filename prompts q is typed; Esc cancels",
                    "",
                    "? /  help · Shift+I info · q close/quit · Ctrl+C quit",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        Some(Popup::MontageHelp) => {
            lines.extend(
                [
                    "MONTAGE · figure preparation",
                    "  p   Return to single slice     P   Reset preset (3×4)",
                    "  B / E    Edit inclusive start / stop (zero-based)",
                    "  Z / F    Edit columns / rows (zoom and grid)",
                    "  U        Edit slice interval (0 = 12 evenly spaced)",
                    "  D        Edit tile gap (pixels)",
                    "  Type value or ↑↓ to adjust; Enter apply; Esc cancel",
                    "  +/- zoom   [/] interval   </> gap (quick adjustment)",
                    "  ↑↓ or wheel    Move rows; ←→ move by one slice",
                    "  PgUp/PgDn a screen; Home/End first/last slice",
                    "  N   Toggle slice numbers (off by default)",
                    "  C   Toggle IDs for a visible categorical layer",
                    "  Tab   Image only (no UI text)",
                    "  g   Edge AA    G   Atlas Gaussian blur (0..4 px)",
                    "  a   Atlas on/off    O   Atlas opacity",
                    "  1–9 select layer · e window · c palette · o opacity",
                    "  Projection toggles are disabled in montage mode",
                    "  n   Load images    t   Layer list    s   Save scene",
                    "  In layer list: ↑↓ / jk select; Enter / q / Esc close",
                    "  I   Info   q / ? / Esc close help · Ctrl+C quit",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        Some(Popup::Info) => {
            lines.push(format!("Original base image  {}", app.path));
            lines.push(format!(
                "Grid       {} × {} × {} voxels",
                vol.dims()[0],
                vol.dims()[1],
                vol.dims()[2]
            ));
            lines.push(format!(
                "Spacing    {:.4} × {:.4} × {:.4}",
                vol.spacing[0], vol.spacing[1], vol.spacing[2]
            ));
            lines.push(format!(
                "Stored     {} (NIfTI code {}) · decoded f32",
                vol.datatype_name(),
                vol.datatype
            ));
            lines.push(format!(
                "Data       {} .. {}",
                compact_number(base_range.0),
                compact_number(base_range.1)
            ));
            lines.push(format!(
                "Window     {} .. {}",
                compact_number(app.window.0),
                compact_number(app.window.1)
            ));
            lines.push(format!(
                "Axis       {} slice {}/{}  · rotation {}° · stride {}",
                app.axis.label(),
                app.index + 1,
                vol.depth(app.axis),
                app.rotation,
                app.stride
            ));
            lines.push(format!(
                "Opacity    {:.0}%{}",
                app.base_opacity * 100.0,
                if app.base_visible { "" } else { " · hidden" }
            ));
            lines.push(format!("Projection {}", app.projection.label()));
            lines.push(format!(
                "Background {}",
                if app.transparent_background {
                    "RGBA"
                } else {
                    "RGB"
                }
            ));
            if app.montage.enabled {
                lines.push(format!(
                    "Montage   {}..{} every {} | {} cols | gap {} px",
                    app.montage.start,
                    app.montage.stop,
                    app.montage.interval,
                    app.montage.columns,
                    app.montage.gap
                ));
            }
            if let Some(atlas) = &layers.atlas {
                lines.push(String::new());
                lines.push(format!(
                    "Atlas      {}",
                    app.atlas_name.as_deref().unwrap_or("(loaded)")
                ));
                lines.push(format!(
                    "Atlas      {:.0}% opacity · {} outline · antialias {}",
                    atlas.opacity * 100.0,
                    if atlas.wireframe {
                        "wireframe"
                    } else {
                        "solid label"
                    },
                    if app.atlas_aa && !atlas.wireframe {
                        "on"
                    } else {
                        "off"
                    }
                ));
                lines.push(format!("Atlas blur {:.2} px sigma", app.atlas_blur));
                lines.push(format!("LUT        {} region colors", atlas.regions.len()));
            }
            lines.push(String::new());
            lines.push("Image stack (top -> bottom; atlas above images)".into());
            for pos in (0..=layers.overlays.len()).rev() {
                let id = app.layer_at(pos);
                if id == 0 {
                    lines.push(format!(
                        "  {} {}  {}  data {}..{}",
                        pos + 1,
                        app.path,
                        vol.datatype_name(),
                        compact_number(base_range.0),
                        compact_number(base_range.1)
                    ));
                } else {
                    let layer = &layers.overlays[id - 1];
                    lines.push(format!(
                        "  {} {}  {}  data {}..{}",
                        pos + 1,
                        layer.name,
                        layer.vol.datatype_name(),
                        compact_number(layer.data_range.0),
                        compact_number(layer.data_range.1)
                    ));
                }
            }
            lines.push(String::new());
            lines.push("q / Esc / I / ?  close · Ctrl+C quit".into());
        }
        None => return Ok(()),
    }
    let width = (cols as usize).saturating_sub(4).min(78).max(12);
    let content_w = width.saturating_sub(4);
    let max_lines = rows.saturating_sub(6) as usize;
    let showing = lines.len().min(max_lines);
    let first = app.popup_scroll.min(lines.len().saturating_sub(showing));
    let block_h = showing + 4;
    let left = cols.saturating_sub(width as u16) / 2;
    let top = rows.saturating_sub(block_h as u16) / 2;
    let bg = Color::Rgb {
        r: 36,
        g: 36,
        b: 48,
    };
    let fg = Color::Grey;
    let border_color = Color::DarkGrey;
    let title = match app.popup {
        Some(Popup::Help) => " Help · ↑↓ scroll · q/Esc close ",
        Some(Popup::MontageHelp) => " Montage help · ↑↓ scroll · q/Esc close ",
        _ => " Image information · ↑↓ scroll · q/Esc close ",
    };
    let bar = "─".repeat(width.saturating_sub(2));
    queue!(
        out,
        MoveTo(left, top),
        SetForegroundColor(border_color),
        Print(format!("╭{bar}╮")),
        ResetColor
    )?;
    // Put title inside border without changing the panel width.
    queue!(
        out,
        MoveTo(left + 2, top),
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    for row in 0..showing + 2 {
        let y = top + row as u16 + 1;
        queue!(
            out,
            MoveTo(left, y),
            SetForegroundColor(border_color),
            Print("│"),
            SetBackgroundColor(bg),
            Print(" ".repeat(width.saturating_sub(2))),
            ResetColor,
            SetForegroundColor(border_color),
            Print("│"),
            ResetColor
        )?;
        if row < showing {
            let shown = &lines[first + row];
            let text = crop(shown, content_w);
            queue!(
                out,
                MoveTo(left + 2, y),
                SetBackgroundColor(bg),
                SetForegroundColor(
                    if ["NAVIGATION", "LAYERS", "MONTAGE"]
                        .iter()
                        .any(|heading| shown.starts_with(heading))
                    {
                        Color::Yellow
                    } else {
                        fg
                    }
                ),
                Print(text),
                ResetColor
            )?;
        }
    }
    queue!(
        out,
        MoveTo(left, top + showing as u16 + 3),
        SetForegroundColor(border_color),
        Print(format!("╰{bar}╯")),
        ResetColor
    )?;
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
    cache: &mut Option<render::SamplingMap>,
    stats: &mut FrameStats,
    previous: Option<ViewLayout>,
) -> Result<ViewLayout> {
    let (cols, rows) = terminal::size()?;
    if app.popup.is_some() {
        frames.clear(out)?;
        draw_popup(out, vol, layers, app, base_range)?;
        return Ok(layout(vol, app, layers, cols, rows));
    }
    if app
        .edit
        .as_ref()
        .is_some_and(|e| e.field == EditField::AddPath)
    {
        frames.clear(out)?;
        draw_picker(out, app)?;
        return Ok(layout(vol, app, layers, cols, rows));
    }
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
    if previous.is_some_and(|old| layout_changed(old, view)) {
        frames.clear(out)?;
        queue!(out, Clear(ClearType::All))?;
    }
    let t = Instant::now();
    let pixels = if app.montage.enabled {
        let extra = usize::from(app.edit.is_some() || app.notice.is_some());
        let head = if app.fullscreen { 0 } else { 2 };
        let footer = if app.fullscreen {
            0
        } else {
            1 + view.list_count + extra
        };
        let plan = montage::plan(vol, app, cols, rows, head + footer);
        if cache
            .as_ref()
            .is_none_or(|map| !map.matches(vol, app, plan.tile_width, plan.tile_height))
        {
            *cache = Some(render::SamplingMap::new(
                vol,
                app,
                plan.tile_width,
                plan.tile_height,
            ));
        }
        montage::composite(vol, app, layers, &plan, cache.as_ref().unwrap())
    } else {
        if cache
            .as_ref()
            .is_none_or(|map| !map.matches(vol, app, view.geom.width, view.geom.height))
        {
            *cache = Some(render::SamplingMap::new(
                vol,
                app,
                view.geom.width,
                view.geom.height,
            ));
        }
        render::composite_with_map(
            vol,
            app,
            layers,
            cache.as_ref().unwrap(),
            app.transparent_background,
        )
    };
    let raster_time = t.elapsed();
    let t = Instant::now();
    let frame = kitty::encode_pixels(&pixels, app.transparent_background)?;
    let encode_time = t.elapsed();
    let t = Instant::now();
    queue!(out, MoveTo(view.x, view.y))?;
    let next = frames.allocate();
    kitty::replace(out, next, &frame, view.geom, frames.active_id)?;
    frames.active_id = Some(next);
    draw_ui(out, vol, layers, app, base_range, view)?;
    out.flush()?;
    stats.frames += 1;
    stats.raster += raster_time;
    stats.encode += encode_time;
    stats.output += t.elapsed();
    stats.compressed_bytes += frame.bytes;
    Ok(view)
}

fn mouse_montage_tile(
    vol: &Volume,
    app: &App,
    view: ViewLayout,
    mouse: MouseEvent,
    pixel_mouse: bool,
) -> Option<(usize, (usize, usize))> {
    let (cols, rows) = terminal::size().ok()?;
    let (cw, ch) = render::cell_pixels(cols, rows);
    let (rx, ry) = mouse_raster(view, mouse, pixel_mouse, cw, ch)?;
    let extra = usize::from(app.edit.is_some() || app.notice.is_some());
    let head = if app.fullscreen { 0 } else { 2 };
    let footer = if app.fullscreen {
        0
    } else {
        1 + view.list_count + extra
    };
    let plan = montage::plan(vol, app, cols, rows, head + footer);
    for tile in &plan.tiles {
        if rx >= tile.x
            && ry >= tile.y
            && rx < tile.x + plan.tile_width
            && ry < tile.y + plan.tile_height
        {
            let probe = render::voxel_at_raster(
                vol,
                app,
                rx - tile.x,
                ry - tile.y,
                plan.tile_width,
                plan.tile_height,
            )?;
            return Some((tile.slice, probe));
        }
    }
    None
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
    let map = render::SamplingMap::new(vol, app, geom.width, geom.height);
    let mut rgb_time = Duration::ZERO;
    let mut encode_time = Duration::ZERO;
    let mut compressed = 0;
    for i in 0..count {
        app.index = i * vol.depth(app.axis) / count;
        let t = Instant::now();
        let pixels = render::composite_with_map(vol, app, layers, &map, app.transparent_background);
        rgb_time += t.elapsed();
        let t = Instant::now();
        compressed += kitty::encode_pixels(&pixels, app.transparent_background)?.bytes;
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

/// Drop *already queued* autorepeats of the key whose slice has just
/// advanced. Without this, an expensive frame can leave dozens of repeats in
/// Crossterm's queue, causing scrolling to continue long after key release.
/// Preserve the first different input so a direction change or Quit is never
/// swallowed. Applies to keyboard navigation only; wheel events retain their
/// native number of steps, and probe movement remains precise.
fn is_slice_key(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind};
    key.kind != KeyEventKind::Release
        && matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Char('h' | 'j' | 'k' | 'l' | 'H' | 'J' | 'K' | 'L')
        )
}
fn same_navigation_key(a: &crossterm::event::KeyEvent, b: &crossterm::event::KeyEvent) -> bool {
    is_slice_key(a) && a.code == b.code && a.modifiers == b.modifiers
}
fn drop_stale_key_repeats(
    key: crossterm::event::KeyEvent,
    next_event: &mut Option<Event>,
) -> Result<usize> {
    use crossterm::event::KeyEventKind;
    let mut dropped = 0usize;
    // Read only events that are already waiting; never block for new repeats.
    for _ in 0..4096 {
        if !event::poll(Duration::ZERO)? {
            break;
        }
        let evt = event::read()?;
        match evt {
            Event::Key(repeated) if same_navigation_key(&key, &repeated) => {
                if repeated.kind == KeyEventKind::Release {
                    break;
                }
                dropped += 1;
            }
            other => {
                *next_event = Some(other);
                break;
            }
        }
    }
    Ok(dropped)
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
    let base_range = vol.data_range();
    let window = config.window.unwrap_or_else(|| vol.auto_window());
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
    app.pixel_budget = config.pixel_budget;
    app.atlas_aa = config.atlas_aa;
    app.atlas_blur = config.atlas_blur;
    app.base_opacity = config.base_opacity;
    app.transparent_background = config.transparent_background;
    app.default_window = vol.auto_window();
    app.atlas_name = config.atlas_path.as_ref().map(|p| layers::short_name(p));
    app.sync_layer_order(layers.overlays.len());
    app.reset_montage(&vol);
    if let Some(scene) = config.scene.as_ref() {
        scene.apply_to_loaded(&mut app, &mut layers)?;
    }
    app.normalize_montage(&vol);
    if let Some(count) = config.benchmark_frames {
        return benchmark(&vol, &layers, &mut app, count);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("Interactive graphics require a terminal; use --bench N for a headless test");
    }
    let _guard = TerminalGuard::enter(config.enhanced_keys, config.mouse_pixels)?;
    let mut out = io::stdout().lock();
    let mut frames = FrameState::new();
    let mut cache: Option<render::SamplingMap> = None;
    let mut stats = FrameStats::default();
    queue!(out, Clear(ClearType::All))?;
    let mut view = draw(
        &mut out,
        &vol,
        &layers,
        &app,
        base_range,
        &mut frames,
        &mut cache,
        &mut stats,
        None,
    )?;
    let frame_interval = Duration::from_secs_f64(1.0 / config.max_fps as f64);
    let mut last_draw = Instant::now();
    let mut pending_frame = false;
    let mut pending_ui = false;
    let mut pending_layout = false;
    let mut deferred_event: Option<Event> = None;
    // The candidate region is updated immediately on hover; graphics are only
    // redrawn if the candidate differs from the active highlight after a delay.
    let mut hover_candidate: Option<i32> = None;
    let mut hover_deadline: Option<Instant> = None;
    let hover_delay = Duration::from_millis(config.hover_delay_ms);
    loop {
        let mut wait = if pending_frame || pending_ui || pending_layout {
            frame_interval.saturating_sub(last_draw.elapsed())
        } else {
            Duration::from_millis(150)
        };
        if let Some(deadline) = hover_deadline {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        // Present the pending slice before accepting another navigation key.
        // Drop queued repeats of the *same* key (see helper above) so they
        // cannot continue moving slices long after the user releases it.
        if pending_frame || pending_layout {
            let remaining = frame_interval.saturating_sub(last_draw.elapsed());
            if !remaining.is_zero() {
                std::thread::sleep(remaining);
            }
        } else if deferred_event.is_some() || event::poll(wait)? {
            for _ in 0..256 {
                let mut navigated = false;
                let evt = if let Some(saved) = deferred_event.take() {
                    saved
                } else {
                    event::read()?
                };
                match evt {
                    Event::Key(key) => {
                        if app.notice.take().is_some() {
                            pending_layout = true;
                            pending_frame = true;
                        }
                        let before_index = app.index;
                        let action = app.key(&vol, &mut layers, key.clone());
                        navigated = before_index != app.index;
                        if matches!(action, Action::Redraw | Action::Layout) {
                            // Slice/axis change: update the region associated with
                            // the cursor; don't retain an outline from another slice.
                            if app.projection != app::ProjectionMode::Slice
                                || app.montage.enabled
                                || app.inspect_mode
                                || app.probe.is_none()
                                || !app.show_atlas
                            {
                                app.highlighted_region = None;
                                hover_candidate = None;
                                hover_deadline = None;
                            } else if config.hover_delay_ms != 0 {
                                let target = app.probe.and_then(|(h, v)| {
                                    layers.atlas_id_at(app.axis, app.index, h, v)
                                });
                                if target != hover_candidate {
                                    hover_candidate = target;
                                    hover_deadline = Some(Instant::now() + hover_delay);
                                }
                            }
                        }
                        match action {
                            Action::Quit => {
                                frames.clear(&mut out)?;
                                out.flush()?;
                                drop(out);
                                drop(_guard);
                                if config.profile {
                                    println!("{}", stats.report());
                                }
                                return Ok(());
                            }
                            Action::Redraw => pending_frame = true,
                            Action::Ui => {
                                let (cols, rows) = terminal::size()?;
                                let updated = layout(&vol, &app, &layers, cols, rows);
                                if layout_changed(view, updated) {
                                    pending_layout = true;
                                    pending_frame = true;
                                } else {
                                    pending_ui = true;
                                }
                            }
                            Action::Layout => {
                                pending_layout = true;
                                pending_frame = true;
                            }
                            Action::SaveScene(path) => {
                                match scene::save(&path, &config, &app, &layers) {
                                    Ok(()) => {
                                        app.scene_path = Some(
                                            std::fs::canonicalize(&path).unwrap_or(path.clone()),
                                        );
                                        app.notice =
                                            Some(format!("Scene saved: {}", path.display()));
                                    }
                                    Err(err) => {
                                        app.notice = Some(format!("Scene save failed: {err:#}"));
                                    }
                                }
                                pending_ui = true;
                            }
                            Action::LoadOverlays(paths) => {
                                let mut loaded = 0usize;
                                let mut failed = Vec::new();
                                frames.clear(&mut out)?;
                                let total = paths.len();
                                for (i, path) in paths.into_iter().enumerate() {
                                    app.notice = Some(format!(
                                        "Loading {}/{}: {} ...",
                                        i + 1,
                                        total,
                                        path.display()
                                    ));
                                    // Enter leaves the picker and its marks available until I/O completes.
                                    draw_picker(&mut out, &app)?;
                                    match layers.add_overlay(&path, &vol) {
                                        Ok(_) => {
                                            loaded += 1;
                                        }
                                        Err(err) => {
                                            failed.push(format!("{}: {err:#}", path.display()));
                                        }
                                    }
                                }
                                app.edit = None;
                                app.sync_layer_order(layers.overlays.len());
                                if loaded > 0 {
                                    app.selected = app.layer_order.len() - 1;
                                }
                                app.notice = Some(if failed.is_empty() {
                                    format!(
                                        "Loaded {loaded} image{}",
                                        if loaded == 1 { "" } else { "s" }
                                    )
                                } else {
                                    format!(
                                        "Loaded {loaded}; {} failed: {}",
                                        failed.len(),
                                        failed[0]
                                    )
                                });
                                pending_layout = true;
                                pending_frame = true;
                            }
                            Action::None => {}
                        }
                        if navigated && !app.inspect_mode && is_slice_key(&key) {
                            drop_stale_key_repeats(key, &mut deferred_event)?;
                        }
                    }
                    Event::Resize(_, _) => {
                        pending_layout = true;
                        pending_frame = true;
                        last_draw = Instant::now() - frame_interval;
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            if app.edit.is_none() && app.popup.is_none() =>
                        {
                            let direction = if mouse.kind == MouseEventKind::ScrollUp {
                                1
                            } else {
                                -1
                            };
                            let multiplier = App::multiplier(mouse.modifiers, false);
                            let delta = if app.montage.enabled {
                                -direction * (app.montage.columns * multiplier) as isize
                            } else {
                                direction * (app.stride * multiplier) as isize
                            };
                            if app.move_slices(&vol, delta) {
                                pending_frame = true;
                                navigated = true;
                            }
                        }
                        MouseEventKind::Down(_)
                            if app.montage.enabled && app.edit.is_none() && app.popup.is_none() =>
                        {
                            if let Some((slice, probe)) =
                                mouse_montage_tile(&vol, &app, view, mouse, config.mouse_pixels)
                            {
                                app.index = slice;
                                app.probe = Some(probe);
                                pending_frame = true;
                            }
                        }
                        MouseEventKind::Moved
                        | MouseEventKind::Drag(_)
                        | MouseEventKind::Down(_)
                            if !app.montage.enabled
                                && !app.inspect_mode
                                && app.edit.is_none()
                                && app.popup.is_none() =>
                        {
                            let next = mouse_voxel(&vol, &app, view, mouse, config.mouse_pixels);
                            if app.probe != next {
                                app.probe = next;
                                pending_ui = true;
                            }
                            if app.projection == app::ProjectionMode::Slice
                                && config.hover_delay_ms != 0
                                && app.show_atlas
                                && layers.atlas.is_some()
                            {
                                let target = next.and_then(|(h, v)| {
                                    layers.atlas_id_at(app.axis, app.index, h, v)
                                });
                                if target != hover_candidate {
                                    hover_candidate = target;
                                    hover_deadline = Some(Instant::now() + hover_delay);
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                if navigated || (deferred_event.is_none() && !event::poll(Duration::ZERO)?) {
                    break;
                }
            }
        }
        if hover_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            hover_deadline = None;
            if app.highlighted_region != hover_candidate {
                app.highlighted_region = hover_candidate;
                pending_frame = true;
            }
        }
        if (pending_frame || pending_ui || pending_layout) && last_draw.elapsed() >= frame_interval
        {
            // Account for raster+transfer time when pacing frames: the next
            // frame interval starts at draw BEGIN, not after slow I/O finishes.
            let draw_started = Instant::now();
            if pending_layout {
                frames.clear(&mut out)?;
                queue!(out, Clear(ClearType::All))?;
            }
            if pending_frame {
                let previous = if pending_layout { None } else { Some(view) };
                view = draw(
                    &mut out,
                    &vol,
                    &layers,
                    &app,
                    base_range,
                    &mut frames,
                    &mut cache,
                    &mut stats,
                    previous,
                )?;
            } else if pending_ui {
                let (cols, rows) = terminal::size()?;
                let updated = layout(&vol, &app, &layers, cols, rows);
                if layout_changed(view, updated) {
                    view = draw(
                        &mut out,
                        &vol,
                        &layers,
                        &app,
                        base_range,
                        &mut frames,
                        &mut cache,
                        &mut stats,
                        Some(view),
                    )?;
                } else {
                    view = updated;
                    draw_ui(&mut out, &vol, &layers, &app, base_range, view)?;
                }
            }
            pending_frame = false;
            pending_ui = false;
            pending_layout = false;
            last_draw = draw_started;
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
    fn categorical_filename_gradient_is_deterministic_and_bounded() {
        let name = "entactogen_cluster_é_index_0123456789";
        let runs = rainbow_runs(name);
        assert_eq!(runs, rainbow_runs(name));
        assert_eq!(
            runs.iter()
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .concat(),
            name
        );
        assert_eq!(runs.len(), 12);
        assert_ne!(runs.first().unwrap().0, runs.last().unwrap().0);
        assert!(rainbow_runs("").is_empty());
        assert_eq!(rainbow_runs("x").len(), 1);
    }

    #[test]
    fn identifies_only_slice_navigation_as_repeatable() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        assert!(is_slice_key(&KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE
        )));
        assert!(is_slice_key(&KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE
        )));
        assert!(!is_slice_key(&KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
        assert!(!is_slice_key(&KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::SHIFT
        )));
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let down_repeat = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let modified = KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT);
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(same_navigation_key(&down, &down_repeat));
        assert!(!same_navigation_key(&down, &up));
        assert!(!same_navigation_key(&down, &modified));
        assert!(!same_navigation_key(&down, &quit));
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

#[cfg(test)]
mod layer_list_order_tests {
    use super::top_first_position;

    #[test]
    fn first_list_row_is_topmost_visible_image() {
        assert_eq!(
            (0..3)
                .map(|row| top_first_position(0, 3, row))
                .collect::<Vec<_>>(),
            vec![2, 1, 0]
        );
        // A six-row window within a larger stack is still top-first.
        assert_eq!(
            (0..3)
                .map(|row| top_first_position(5, 3, row))
                .collect::<Vec<_>>(),
            vec![7, 6, 5]
        );
    }
}

#[cfg(test)]
mod layout_invalidation_tests {
    use super::*;
    #[test]
    fn footer_row_changes_invalidate_previous_frame() {
        let old = ViewLayout {
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
        let mut new = old;
        new.footer += 1; // editor closed; previous footer is now inside viewer area
        assert!(layout_changed(old, new));
        new = old;
        new.geom.rows += 1;
        assert!(layout_changed(old, new));
        new = old;
        new.x += 1;
        assert!(layout_changed(old, new));
        new = old;
        new.list_start += 1;
        assert!(!layout_changed(old, new)); // scrolling the text-only list
        assert!(!layout_changed(old, old)); // regular slice redraw
    }
}
