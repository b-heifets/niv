//! NIfTI slice viewer. Direct Kitty graphics, no libghostty or Rex dependency.
mod app;
mod config;
mod edit;
mod keymap;
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
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyModifiers, KeyboardEnhancementFlags, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
        // XTSHIFTESCAPE: request Shift mouse reporting while this TUI runs.
        // Unsupported terminals may ignore it. Reset on exit.
        let _ = io::stdout().write_all(b"\x1b[>1s");
        let _ = io::stdout().flush();
        if mouse_pixels {
            io::stdout().write_all(b"\x1b[?1016h")?;
            io::stdout().flush()?;
        }
        // Enable paste events only after terminal initialization succeeds.
        let guard = Self {
            enhanced_keys,
            mouse_pixels,
        };
        execute!(io::stdout(), EnableBracketedPaste)?;
        Ok(guard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        if self.mouse_pixels {
            let _ = out.write_all(b"\x1b[?1016l");
        }
        let _ = out.write_all(b"\x1b[>0s");
        let _ = execute!(out, DisableBracketedPaste, DisableMouseCapture);
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
    y_offset: usize,
    footer: u16,
    head: u16,
    list_start: usize,
    list_count: usize,
}
// Reserve a stable header footprint from layer ranges, not the hovered value.
// This avoids resizing and retransmitting the image during mouse movement.
const HEADER_GUTTER_ROWS: usize = 3;

fn centered_header(app: &App, layers: &Layers) -> bool {
    layers.atlas.is_some() || app.projection != app::ProjectionMode::Slice || app.montage.enabled
}

fn header_bands(cols: u16, centered: bool) -> (&'static str, usize, usize, usize) {
    let width = cols as usize;
    let hint = if width >= 52 { "Press ? for help" } else { "?" };
    let hint_x = width.saturating_sub(hint.len());
    // Reserve a symmetric central band, leaving space for the right hint.
    let center_width = if centered {
        (width / 2).min(width.saturating_sub(2 * (hint.len() + 1)))
    } else {
        0
    };
    let first_width = if centered {
        (width - center_width) / 2
    } else {
        hint_x
    };
    (hint, hint_x, center_width, first_width.saturating_sub(1))
}

// Keep each coordinate or layer value together where possible. The slots
// correspond to the top-left band, full-width continuation rows, and the
// unused gutter beside the image. No text is drawn over the graphics.
fn pack_header(parts: &[String], widths: &[usize]) -> (Vec<String>, usize) {
    use unicode_width::UnicodeWidthStr;
    let mut lines = Vec::with_capacity(widths.len());
    let mut next = 0;
    for &width in widths {
        let mut line = String::new();
        while let Some(part) = parts.get(next) {
            let sep = usize::from(!line.is_empty());
            if line.width() + sep + part.width() > width {
                break;
            }
            if sep != 0 {
                line.push(' ');
            }
            line.push_str(part);
            next += 1;
        }
        lines.push(line);
    }
    (lines, next)
}

fn header_widths(first: usize, head: u16, cols: u16, x: u16, image_rows: u16) -> Vec<usize> {
    let mut widths = vec![first];
    // Extra rows are allocated only when the readout cannot fit the gutter.
    widths.extend(std::iter::repeat_n(
        cols as usize,
        head.saturating_sub(1) as usize,
    ));
    widths.extend(std::iter::repeat_n(
        x.saturating_sub(1) as usize,
        HEADER_GUTTER_ROWS.min(image_rows as usize),
    ));
    widths
}

fn header_estimate(vol: &Volume, app: &App, layers: &Layers) -> Vec<String> {
    let dims = vol.dims();
    let mut parts = vec![format!("[{},{},{}]", dims[0] - 1, dims[1] - 1, dims[2] - 1)];
    for pos in (0..=layers.overlays.len()).rev() {
        let id = app.layer_at(pos);
        if !app.layer_visible(layers, id) {
            continue;
        }
        let range = if id == 0 {
            app.default_window
        } else {
            layers.overlays[id - 1].data_range
        };
        // Allow fractional values even in integer-stored NIfTIs (slope/intercept
        // and average projection can produce them). Bound large values using
        // the same scientific-notation policy as the live readout.
        let magnitude = range.0.abs().max(range.1.abs());
        let n = if magnitude >= 1e7 {
            16
        } else {
            format!("-{magnitude:.7}").len().max(16)
        };
        parts.push(format!(
            "| {}{}: {}",
            app.layer_number(pos),
            projection_tag(app, id),
            "0".repeat(n)
        ));
    }
    parts
}

fn layout(vol: &Volume, app: &App, layers: &Layers, cols: u16, rows: u16) -> ViewLayout {
    let extra = usize::from(app.edit.is_some() || app.notice.is_some());
    let list_count = if app.fullscreen || !app.show_layer_list {
        0
    } else {
        (layers.overlays.len() + 1)
            .min((rows as usize) / 2)
            .min((rows as usize).saturating_sub(11 + extra))
    };
    let list_start = (app.selected + 1)
        .saturating_sub(list_count)
        .min(layers.overlays.len() + 1 - list_count);
    let footer_rows = if app.fullscreen {
        0u16
    } else {
        3 + layer_list_rows(list_count) as u16 + extra as u16
    };
    let mut head = if app.fullscreen { 0u16 } else { 1 };
    let estimate = if app.fullscreen {
        Vec::new()
    } else {
        header_estimate(vol, app, layers)
    };
    let (_, _, _, first) = header_bands(cols, centered_header(app, layers));
    // Cap header growth so small panes always retain a usable image area.
    let max_head = rows.saturating_sub(footer_rows + 1).max(head).min(8);
    loop {
        let reserved = (head + footer_rows) as usize;
        let available = rows.saturating_sub(head + footer_rows);
        let cells = render::cell_pixels(cols, rows);
        // Keep the half-cell top inset, but reclaim the full row previously
        // reserved for it. Kitty's placement rectangle ends before the footer.
        let y_offset = if !app.fullscreen && available > 2 {
            cells.1 / 2
        } else {
            0
        };
        let geom = if app.montage.enabled {
            montage::plan_with_cells(vol, app, cols, rows, reserved, cells).geom
        } else {
            render::geometry_with_inset(vol, app, cols, rows, reserved, cells, y_offset)
        };
        let x = cols.saturating_sub(geom.cols) / 2;
        let spare = available.saturating_sub(geom.rows);
        let y = head + spare / 2;
        if app.fullscreen
            || head >= max_head
            || pack_header(&estimate, &header_widths(first, head, cols, x, geom.rows)).1
                == estimate.len()
        {
            return ViewLayout {
                geom,
                x,
                y,
                y_offset,
                footer: rows.saturating_sub(footer_rows),
                head,
                list_start,
                list_count,
            };
        }
        head += 1;
    }
}

fn layer_list_rows(count: usize) -> usize {
    count + usize::from(count > 0)
}

// A change in reserved UI rows or image placement must clear the old text
// even when the originating action was a simple raster redraw. The list's
// scroll offset is purely textual and does not invalidate the image.
fn layout_changed(old: ViewLayout, new: ViewLayout) -> bool {
    old.x != new.x
        || old.y != new.y
        || old.y_offset != new.y_offset
        || old.footer != new.footer
        || old.head != new.head
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

fn integer_display(v: f32) -> Option<String> {
    // Don't round nearly-one probability thresholds to 1. Only exact stored
    // integers get the short form; normalize negative zero for display.
    if v.is_finite() && v.fract() == 0. && v.abs() < 1e12 {
        Some(if v == 0. {
            "0".into()
        } else {
            format!("{v:.0}")
        })
    } else {
        None
    }
}
fn intensity(v: f32) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    integer_display(v).unwrap_or_else(|| {
        if v.abs() >= 1e7 || (v != 0. && v.abs() < 1e-7) {
            format!("{v:.7e}")
        } else {
            format!("{v:.7}")
        }
    })
}
fn compact_number(v: f32) -> String {
    if let Some(s) = integer_display(v) {
        return s;
    }
    if v.is_finite() && v.abs() < 100_000.0 {
        // Avoid displaying a fractional threshold as an integer after rounding.
        let short = format!("{v:.5}");
        if short.parse::<f32>().is_ok_and(|n| n.fract() == 0.) {
            v.to_string()
        } else {
            short
        }
    } else {
        format!("{v:.5e}")
    }
}
fn range_edit_hint(field: EditField, window: (f32, f32), data: (f32, f32)) -> String {
    let (lo, hi) = window;
    let (data_lo, data_hi) = data;
    let current = match field {
        EditField::Minimum => format!("{lo}"),
        EditField::Maximum => format!("{hi}"),
        _ => format!("{lo},{hi}"),
    };
    format!("Data: {data_lo}..{data_hi} | current: {current}")
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
// Each displayed character receives its own interpolated RGB color.
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
    let n = chars.len();
    (0..n)
        .map(|i| {
            let t = i as f32 * 6.0 / n.saturating_sub(1).max(1) as f32;
            let k = (t.floor() as usize).min(5);
            let f = t - k as f32;
            let rgb = std::array::from_fn(|c| {
                (STOPS[k][c] as f32 * (1.0 - f) + STOPS[k + 1][c] as f32 * f).round() as u8
            });
            (rgb_color(rgb), chars[i].to_string())
        })
        .collect()
}

fn styled_layer_line(
    out: &mut impl Write,
    row: u16,
    prefix: &str,
    name: &str,
    suffix: &str,
    cols: u16,
    accent: Color,
    visible: bool,
    rainbow: bool,
    active: bool,
) -> Result<()> {
    use unicode_width::UnicodeWidthStr;
    let capacity = cols.saturating_sub(4) as usize;
    let prefix = picker_clip(prefix, capacity, false);
    let suffix = picker_clip(suffix, capacity.saturating_sub(prefix.width()), false);
    let name = picker_clip(
        name,
        capacity.saturating_sub(prefix.width() + suffix.width()),
        true,
    );
    let muted = if visible {
        PICKER_MUTED
    } else {
        Color::DarkGrey
    };
    queue!(
        out,
        MoveTo(0, row),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(PICKER_MUTED),
        Print("│ "),
        ResetColor
    )?;
    if active {
        queue!(out, SetBackgroundColor(PICKER_HIGHLIGHT))?;
    }
    queue!(
        out,
        SetForegroundColor(if active { PICKER_ACCENT } else { muted }),
        Print(&prefix)
    )?;
    if rainbow && visible {
        for (color, run) in rainbow_runs(&name) {
            queue!(out, SetForegroundColor(color), Print(run))?;
        }
    } else {
        queue!(
            out,
            SetForegroundColor(if visible { accent } else { Color::DarkGrey }),
            Print(&name)
        )?;
    }
    queue!(out, SetForegroundColor(muted), Print(&suffix))?;
    queue!(
        out,
        Print(" ".repeat(capacity.saturating_sub(prefix.width() + name.width() + suffix.width()))),
        ResetColor,
        SetForegroundColor(PICKER_MUTED),
        Print(" │"),
        ResetColor
    )?;
    Ok(())
}

fn projection_tag(app: &App, id: usize) -> &'static str {
    match app.projection_for(id) {
        app::ProjectionMode::Slice => "",
        app::ProjectionMode::Max => " [MAX]",
        app::ProjectionMode::Mean => " [AVG]",
        app::ProjectionMode::MinPositive => " [MIN+]",
    }
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
    base_range: (f32, f32),
    view: ViewLayout,
) -> Result<()> {
    if app.fullscreen {
        return Ok(());
    }
    let (cols, rows) = terminal::size()?;
    if cols < 16 || rows < 8 || view.geom.rows == 0 {
        return Ok(());
    }
    let footer_x = 0u16;
    let (w, h, _) = vol.plane(app.axis);
    let (vh, vv) = app.probe.unwrap_or((w / 2, h / 2));
    let probe_slice = app.probe_index();
    let (hint, hint_x, center_width, _) = header_bands(cols, centered_header(app, layers));
    let (mut label, mut label_color) = (String::new(), PICKER_FG);
    if app.projection != app::ProjectionMode::Slice {
        let outline = match &layers.atlas {
            Some(atlas)
                if app.show_projected_atlas
                    && atlas.opacity > 0.
                    && (!atlas.wireframe || atlas.labels.is_some()) =>
            {
                match app.projected_atlas_detail {
                    0 | 1 if !atlas.regions.values().any(|r| !r.general_region.is_empty()) => {
                        " · regional LUT required"
                    }
                    0 => " · major region outlines",
                    1 => " · broad region outlines",
                    2 => " · atlas silhouette",
                    _ => " · no outlines",
                }
            }
            Some(atlas)
                if app.show_projected_atlas && atlas.wireframe && atlas.labels.is_none() =>
            {
                " · solid atlas required for outlines"
            }
            _ => "",
        };
        label = format!("Projection along {}{outline}", app.axis.label());
    } else if layers.atlas.is_some() && probe_slice.is_some() {
        if let Some((text, color)) = layers.region_details(app.axis, probe_slice.unwrap(), vh, vv) {
            label = text;
            if let Some(rgb) = color {
                label_color = rgb_color(rgb);
            }
        }
    } else if app.montage.enabled {
        label = "Montage".into();
    }
    use unicode_width::UnicodeWidthStr;
    let label = picker_clip(&label, center_width, false);
    let label_x = (cols as usize).saturating_sub(label.width()) / 2;
    let first = (if label.is_empty() {
        hint_x
    } else {
        label_x.min(hint_x)
    })
    .saturating_sub(1);
    let [ix, iy, iz] = app.axis.xyz(probe_slice.unwrap_or(app.index), vh, vv);
    let coords = if app.projection == app::ProjectionMode::Slice {
        format!("[{ix},{iy},{iz}]")
    } else {
        match app.axis {
            volume::Axis::X => format!("[*,{iy},{iz}]"),
            volume::Axis::Y => format!("[{ix},*,{iz}]"),
            volume::Axis::Z => format!("[{ix},{iy},*]"),
        }
    };
    let mut parts = if probe_slice.is_some() {
        vec![coords]
    } else {
        Vec::new()
    };
    for pos in (0..=layers.overlays.len()).rev() {
        let Some(slice) = probe_slice else {
            break;
        };
        let id = app.layer_at(pos);
        if !app.layer_visible(layers, id) {
            continue;
        }
        let source = if id == 0 {
            vol
        } else {
            &layers.overlays[id - 1].vol
        };
        let value = if app.montage.enabled {
            source.sample(app.axis, slice, vh, vv)
        } else {
            render::display_value(source, app, id, vh, vv)
        };
        parts.push(format!(
            "| {}{}: {}",
            app.layer_number(pos),
            projection_tag(app, id),
            intensity(value)
        ));
    }
    let widths = header_widths(first, view.head, cols, view.x, view.geom.rows);
    let (mut text, used) = pack_header(&parts, &widths);
    if used < parts.len() {
        // Only possible in exceptionally small panes / very large stacks.
        // Flag omitted values, rather than writing across the image or help.
        if let Some(i) = widths.iter().rposition(|&n| n >= 2) {
            let prev = std::mem::take(&mut text[i]);
            text[i] = format!(
                "{}…",
                picker_clip(&prev, widths[i] - 1, false).trim_end_matches('…')
            );
        }
    }
    for row in 0..view.head {
        queue!(out, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
    }
    queue!(
        out,
        MoveTo(0, 0),
        SetForegroundColor(Color::Grey),
        Print(&text[0]),
        ResetColor
    )?;
    if !label.is_empty() {
        queue!(
            out,
            MoveTo(label_x as u16, 0),
            SetAttribute(Attribute::Bold),
            SetForegroundColor(label_color),
            Print(&label),
            ResetColor,
            SetAttribute(Attribute::Reset)
        )?;
    }
    queue!(
        out,
        MoveTo(hint_x as u16, 0),
        SetForegroundColor(PICKER_MUTED),
        Print(hint),
        ResetColor
    )?;
    let full_rows = view.head.saturating_sub(1) as usize;
    for i in 0..full_rows {
        queue!(
            out,
            MoveTo(0, (i + 1) as u16),
            SetForegroundColor(Color::Grey),
            Print(&text[i + 1]),
            ResetColor
        )?;
    }
    // Clear only the unused left gutter, never a whole line containing Kitty
    // graphics. This removes stale values without disturbing the image.
    for i in 1 + full_rows..text.len() {
        let width = widths[i];
        if width == 0 {
            continue;
        }
        let row = view.y + (i - 1 - full_rows) as u16;
        queue!(
            out,
            MoveTo(0, row),
            SetForegroundColor(Color::Grey),
            Print(&text[i]),
            Print(" ".repeat(width.saturating_sub(text[i].width()))),
            ResetColor
        )?;
    }

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
        "{} | min: {} | max: {} | α: {:.0}%{}",
        projection_tag(app, app.selected_layer_id()),
        compact_number(lo),
        compact_number(hi),
        opacity * 100.0,
        marker
    );
    let prefix = format!("{}: ", app.layer_number(app.selected));
    let name = selected_name(app, layers);
    let panel_width = cols as usize;
    let title = if view.list_count > 0 {
        " Layers · Tab / ↑↓ select · Shift+↑↓ move · Enter close "
    } else {
        ""
    };
    let title = picker_clip(title, panel_width.saturating_sub(2), false);
    let border = format!(
        "╭{}{}╮",
        title,
        "─".repeat(panel_width.saturating_sub(2 + title.width()))
    );
    line(out, view.footer, &border, cols, PICKER_MUTED, 0)?;
    if view.list_count > 0 && title.starts_with(" Layers") {
        queue!(
            out,
            MoveTo(2, view.footer),
            SetForegroundColor(PICKER_FG),
            SetAttribute(Attribute::Bold),
            Print("Layers"),
            SetAttribute(Attribute::Reset),
            ResetColor
        )?;
    }
    if app.montage.enabled {
        let reserved = (view.head + rows.saturating_sub(view.footer)) as usize;
        let plan = montage::plan(vol, app, cols, rows, reserved);
        let interval = if app.montage.interval == 0 {
            "auto".into()
        } else {
            app.montage.interval.to_string()
        };
        let depth_spacing = vol
            .micrometre_scale()
            .filter(|_| app.montage.count() > 1)
            .map(|scale| {
                let um = if app.montage.interval == 0 {
                    (app.montage.stop - app.montage.start) as f64 / (app.montage.count() - 1) as f64
                } else {
                    app.montage.interval as f64
                } * vol.spacing[app.axis.index()] as f64
                    * scale;
                format!(
                    " ({}{um:.0} µm)",
                    if app.montage.interval == 0 { "≈" } else { "" }
                )
            })
            .unwrap_or_default();
        let page = if plan.pages > 1 {
            format!(" | row {}/{}", plan.page + 1, plan.pages)
        } else {
            String::new()
        };
        let summary = format!(
            "Montage {}–{} | interval {interval}{depth_spacing} | {}×{} | {}/{} shown{page}",
            app.montage.start,
            app.montage.stop,
            plan.rows,
            plan.columns,
            plan.tiles.len(),
            plan.total_slices
        );
        let budget = if app.montage_pixel_budget == 0 {
            "native".to_owned()
        } else {
            format!("{} px", app.montage_pixel_budget)
        };
        styled_layer_line(
            out,
            view.footer + 1,
            "",
            &format!("{summary} | {budget}"),
            "",
            cols,
            PICKER_FG,
            true,
            false,
            false,
        )?;
    } else {
        styled_layer_line(
            out,
            view.footer + 1,
            &prefix,
            name,
            &settings,
            cols,
            rgb_color(accent),
            selected_visible(app, layers),
            app.selected_palette(layers) == layers::Palette::Random,
            false,
        )?;
    }

    if view.list_count > 0 {
        line(
            out,
            view.footer + 2,
            &format!("├{}┤", "─".repeat(panel_width.saturating_sub(2))),
            cols,
            PICKER_MUTED,
            0,
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
        let active = idx == app.selected;
        let prefix = format!(
            " {} {} {}: ",
            if active { "❯" } else { " " },
            if visible { "●" } else { "○" },
            app.layer_number(idx)
        );
        let suffix = format!(
            "{}{}{}",
            projection_tag(app, id),
            if visible { "" } else { " [off]" },
            if app.cut_layer == Some(id) {
                " [cut]"
            } else {
                ""
            }
        );
        styled_layer_line(
            out,
            view.footer + 3 + k as u16,
            &prefix,
            name,
            &suffix,
            cols,
            rgb_color(pal.accent(id)),
            visible,
            pal == layers::Palette::Random,
            active,
        )?;
    }
    line(
        out,
        view.footer + 2 + layer_list_rows(view.list_count) as u16,
        &format!("╰{}╯", "─".repeat(panel_width.saturating_sub(2))),
        cols,
        PICKER_MUTED,
        0,
    )?;
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
                EditField::PixelBudget => "pixel budget (0 = native)",
            };
            let err = edit.error.as_deref().unwrap_or("");
            let hint = if let Some(v) = app.numeric_montage_value(edit.field) {
                format!("current: {v} · ↑↓ adjust")
            } else {
                // Display the shortest round-trippable f32, not the five-decimal footer.
                match edit.field {
                    EditField::Minimum | EditField::Maximum | EditField::Both => {
                        let id = app.selected_layer_id();
                        let (data_lo, data_hi) = if id == 0 {
                            base_range
                        } else {
                            layers.overlays[id - 1].data_range
                        };
                        range_edit_hint(edit.field, (lo, hi), (data_lo, data_hi))
                    }
                    _ => edit
                        .completion
                        .as_ref()
                        .map(|m| m.status())
                        .unwrap_or_default(),
                }
            };
            let cols_available = cols.saturating_sub(footer_x) as usize;
            let hint = middle_ellipsis(
                &hint,
                if matches!(
                    edit.field,
                    EditField::Minimum | EditField::Maximum | EditField::Both
                ) {
                    cols_available
                        .saturating_sub(what.width() + 24)
                        .max(12)
                        .min(hint.width())
                } else {
                    (cols_available / 3).max(12)
                },
            );
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
fn safe_terminal_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

// Use terminal cell widths, not UTF-8 byte counts, for borders and padding.
// This also keeps filenames containing wide Unicode characters inside the panel.
fn picker_clip(text: &str, width: usize, keep_end: bool) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let safe = safe_terminal_text(text);
    if safe.width() <= width {
        return safe;
    }
    if width == 0 {
        return String::new();
    }
    let limit = width - 1;
    let mut used = 0;
    let mut result = String::new();
    if keep_end {
        let mut tail = Vec::new();
        for ch in safe.chars().rev() {
            let n = ch.width().unwrap_or(0);
            if used + n > limit {
                break;
            }
            used += n;
            tail.push(ch);
        }
        result.push('…');
        result.extend(tail.into_iter().rev());
    } else {
        for ch in safe.chars() {
            let n = ch.width().unwrap_or(0);
            if used + n > limit {
                break;
            }
            used += n;
            result.push(ch);
        }
        result.push('…');
    }
    result
}

const PICKER_BORDER: Color = Color::Rgb {
    r: 88,
    g: 91,
    b: 112,
};
const PICKER_FG: Color = Color::Rgb {
    r: 205,
    g: 214,
    b: 244,
};
const PICKER_MUTED: Color = Color::Rgb {
    r: 147,
    g: 153,
    b: 178,
};
const PICKER_ACCENT: Color = Color::Rgb {
    r: 137,
    g: 220,
    b: 235,
};
const PICKER_DIR: Color = Color::Rgb {
    r: 148,
    g: 226,
    b: 213,
};
const PICKER_HIGHLIGHT: Color = Color::Rgb {
    r: 49,
    g: 50,
    b: 68,
};
const PICKER_SEARCH: Color = Color::Rgb {
    r: 249,
    g: 226,
    b: 175,
};

#[derive(Clone, Copy)]
struct PickerPanel {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}
impl PickerPanel {
    fn new(cols: u16, rows: u16) -> Self {
        let width = cols.saturating_sub(if cols >= 80 { 4 } else { 2 }).min(156);
        let height = rows.saturating_sub(2).min(44);
        Self {
            left: (cols - width) / 2,
            top: (rows - height) / 2,
            width,
            height,
        }
    }
    fn inner(self) -> usize {
        self.width.saturating_sub(4) as usize
    }
    fn bottom(self) -> u16 {
        self.top + self.height - 1
    }
    fn frame(self, out: &mut impl Write) -> Result<()> {
        let rule = "─".repeat(self.width.saturating_sub(2) as usize);
        queue!(
            out,
            MoveTo(self.left, self.top),
            SetForegroundColor(PICKER_BORDER),
            Print(format!("╭{rule}╮"))
        )?;
        for y in self.top + 1..self.bottom() {
            queue!(
                out,
                MoveTo(self.left, y),
                Print("│"),
                MoveTo(self.left + self.width - 1, y),
                Print("│")
            )?;
        }
        queue!(
            out,
            MoveTo(self.left, self.bottom()),
            Print(format!("╰{rule}╯")),
            ResetColor
        )?;
        Ok(())
    }
    fn row(
        self,
        out: &mut impl Write,
        y: u16,
        text: &str,
        fg: Color,
        active: bool,
        keep_end: bool,
    ) -> Result<()> {
        use unicode_width::UnicodeWidthStr;
        let text = picker_clip(text, self.inner(), keep_end);
        let padding = self.inner().saturating_sub(text.width());
        queue!(out, MoveTo(self.left + 2, y), SetForegroundColor(fg))?;
        if active {
            queue!(out, SetBackgroundColor(PICKER_HIGHLIGHT))?;
        }
        queue!(out, Print(text), Print(" ".repeat(padding)), ResetColor)?;
        Ok(())
    }
    fn divider(self, out: &mut impl Write, y: u16) -> Result<()> {
        queue!(
            out,
            MoveTo(self.left + 2, y),
            SetForegroundColor(PICKER_BORDER),
            Print("─".repeat(self.inner())),
            ResetColor
        )?;
        Ok(())
    }
    fn title(self, out: &mut impl Write, title: &str) -> Result<()> {
        queue!(out, SetAttribute(Attribute::Bold))?;
        self.row(out, self.top + 1, title, PICKER_FG, false, false)?;
        queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
        Ok(())
    }
    fn search(
        self,
        out: &mut impl Write,
        query: &str,
        cursor: Option<usize>,
        placeholder: &str,
    ) -> Result<()> {
        use unicode_width::UnicodeWidthStr;
        let y = self.top + 3;
        let left = self.left + 2;
        let width = self.inner();
        let rule = "─".repeat(width.saturating_sub(2));
        queue!(
            out,
            MoveTo(left, y),
            SetForegroundColor(PICKER_BORDER),
            Print(format!("╭{rule}╮")),
            MoveTo(left, y + 1),
            Print("│"),
            MoveTo(left + width as u16 - 1, y + 1),
            Print("│"),
            MoveTo(left, y + 2),
            Print(format!("╰{rule}╯")),
            ResetColor
        )?;
        let capacity = width.saturating_sub(4);
        let prefix = "Filter: ";
        let available = capacity.saturating_sub(prefix.width());
        let text = if query.is_empty() {
            picker_clip(&format!("▏{placeholder}"), available, false)
        } else {
            filter_preview(query, cursor, available)
        };
        queue!(
            out,
            MoveTo(left + 2, y + 1),
            SetForegroundColor(PICKER_SEARCH),
            Print(prefix),
            SetForegroundColor(if query.is_empty() {
                PICKER_MUTED
            } else {
                PICKER_SEARCH
            }),
            Print(text),
            ResetColor
        )?;
        Ok(())
    }
}

/// Keep an in-line cursor visible, even in a long filter path.
fn filter_preview(query: &str, cursor: Option<usize>, width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if width == 0 {
        return String::new();
    }
    let at = crate::path_complete::cursor_position(query, cursor);
    let (left, right) = query.split_at(at);
    let space = width - 1;
    let right_width = right
        .width()
        .min(space / 2)
        .max(space.saturating_sub(left.width()));
    let left_width = space.saturating_sub(right_width);
    format!(
        "{}▏{}",
        picker_clip(left, left_width, true),
        picker_clip(right, right_width, false)
    )
}

/// Allocate space to the basename first and dim the remaining directory path.
fn selected_path_parts(path: &str, width: usize) -> (String, String) {
    use unicode_width::UnicodeWidthStr;
    if path.is_empty() {
        return (String::new(), String::new());
    }
    let trimmed = path.trim_end_matches('/');
    let split = trimmed.rfind('/').map_or(0, |i| i + 1);
    let (parent, name) = path.split_at(split);
    let name = picker_clip(name, width, true);
    let parent = picker_clip(parent, width.saturating_sub(name.width()), true);
    (parent, name)
}
fn selected_path_line(
    out: &mut impl Write,
    x: u16,
    y: u16,
    width: usize,
    path: &str,
) -> Result<()> {
    use unicode_width::UnicodeWidthStr;
    let prefix = picker_clip("Selected: ", width, false);
    let (dir, name) = selected_path_parts(path, width.saturating_sub(prefix.width()));
    queue!(
        out,
        MoveTo(x, y),
        SetForegroundColor(PICKER_MUTED),
        Print(prefix),
        Print(dir),
        SetForegroundColor(PICKER_FG),
        Print(name),
        ResetColor
    )?;
    Ok(())
}

// Keep the score/selection marker visible while truncating the directory path.
fn picker_prefixed(prefix: &str, path: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    format!(
        "{prefix}{}",
        picker_clip(path, width.saturating_sub(prefix.width()), true)
    )
}

fn draw_picker(out: &mut impl Write, app: &App) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    // Leave a compact fallback for small Rex/iOS terminal panes.
    if cols < 42 || rows < 22 {
        return draw_picker_plain(out, app);
    }
    queue!(out, Clear(ClearType::All))?;
    let Some(editor) = &app.edit else {
        return Ok(());
    };
    let panel = PickerPanel::new(cols, rows);
    let w = panel.inner();
    let bottom = panel.bottom();
    panel.frame(out)?;
    if let Some(picker) = &app.directory_picker {
        panel.title(out, "Zoxide navigation · choose directory")?;
        panel.search(
            out,
            &editor.text,
            app.picker_cursor,
            "type directory keywords",
        )?;
        panel.row(
            out,
            panel.top + 6,
            &format!(
                "{} / {} directories · {} marked images",
                picker.len(),
                picker.total(),
                editor.checked.len()
            ),
            PICKER_MUTED,
            false,
            false,
        )?;
        // The list and preview share the available rows; spare list rows are
        // reused by the preview when the filter has only a few matches.
        let available = panel.height as usize - 14;
        let list_rows = (available - available / 3).min(picker.len().max(1));
        let preview_rows = available - list_rows;
        let start = picker
            .selected
            .saturating_sub(list_rows.saturating_sub(1))
            .min(picker.len().saturating_sub(list_rows));
        for offset in 0..list_rows {
            let index = start + offset;
            let Some(dir) = picker.get(index) else {
                break;
            };
            let active = index == picker.selected;
            let prefix = format!("{} {:>7.1}  ", if active { "❯" } else { " " }, dir.score);
            let text = picker_prefixed(&prefix, &dir.path.to_string_lossy(), w);
            panel.row(
                out,
                panel.top + 7 + offset as u16,
                &text,
                if active { PICKER_ACCENT } else { PICKER_FG },
                active,
                false,
            )?;
        }
        if picker.len() == 0 {
            panel.row(
                out,
                panel.top + 7,
                picker
                    .source_error
                    .as_deref()
                    .unwrap_or("No matching directories"),
                PICKER_MUTED,
                false,
                false,
            )?;
        }
        let divider = panel.top + 7 + list_rows as u16;
        panel.divider(out, divider)?;
        let heading = picker
            .selected_dir()
            .map(|d| picker_prefixed("Directory contents · ", &d.path.to_string_lossy(), w))
            .unwrap_or_else(|| "Directory contents".into());
        panel.row(out, divider + 1, &heading, PICKER_MUTED, false, false)?;
        if let Some(err) = &picker.preview_error {
            panel.row(out, divider + 2, err, PICKER_SEARCH, false, false)?;
        } else if picker.preview.is_empty() && picker.selected_dir().is_some() {
            panel.row(
                out,
                divider + 2,
                "(No visible entries)",
                PICKER_MUTED,
                false,
                false,
            )?;
        } else {
            let first = picker
                .preview_offset
                .min(picker.preview.len().saturating_sub(preview_rows));
            for (offset, entry) in picker
                .preview
                .iter()
                .skip(first)
                .take(preview_rows)
                .enumerate()
            {
                let label = format!("  {}{}", entry.name, if entry.is_dir { "/" } else { "" });
                panel.row(
                    out,
                    divider + 2 + offset as u16,
                    &label,
                    if entry.is_dir { PICKER_DIR } else { PICKER_FG },
                    false,
                    false,
                )?;
            }
        }
        let first = picker
            .preview_offset
            .min(picker.preview.len().saturating_sub(preview_rows));
        let last = (first + preview_rows).min(picker.preview.len());
        let status = format!(
            "{}–{} of {}{} entries · Ctrl+↑↓ scroll preview",
            if last == 0 { 0 } else { first + 1 },
            last,
            picker.preview.len(),
            if picker.preview_truncated { "+" } else { "" }
        );
        panel.row(out, bottom - 4, &status, PICKER_MUTED, false, false)?;
        panel.divider(out, bottom - 3)?;
        panel.row(
            out,
            bottom - 2,
            app.notice
                .as_deref()
                .or(editor.error.as_deref())
                .unwrap_or("↑↓ / Tab select · PgUp/PgDn page · Enter jump"),
            if app.notice.is_some() {
                PICKER_SEARCH
            } else if editor.error.is_some() {
                Color::Red
            } else {
                PICKER_MUTED
            },
            false,
            false,
        )?;
        panel.row(
            out,
            bottom - 1,
            "Esc / Ctrl+Z back · Ctrl+Y copy path · Ctrl+C quit",
            PICKER_MUTED,
            false,
            false,
        )?;
    } else if let Some(matches) = &editor.completion {
        panel.title(
            out,
            if app.picker_lut_atlas.is_some() {
                "Choose atlas LUT CSV"
            } else if app.picker_template {
                "Load anatomical template"
            } else {
                match (app.picker_atlas, app.recursive_files.is_some()) {
                    (true, true) => "fd · Find atlas",
                    (true, false) => "Load solid label atlas",
                    (false, true) => "fd · Find NIfTI images recursively",
                    (false, false) => "Load NIfTI images",
                }
            },
        )?;
        panel.search(
            out,
            &editor.text,
            app.picker_cursor,
            "type any filename substring",
        )?;
        let directory = if matches.directory.is_empty() {
            "."
        } else {
            &matches.directory
        };
        let prefix = if let Some(atlas) = &app.picker_lut_atlas {
            format!(
                "{} matches · Atlas: {} · ",
                matches.values.len(),
                layers::short_name(atlas)
            )
        } else if app.picker_atlas {
            format!(
                "{} matches · LUT: {} · ",
                matches.values.len(),
                if app.picker_atlas_lut {
                    "configured"
                } else {
                    "auto / IDs"
                }
            )
        } else if app.picker_template {
            format!("{} matches · bottom layer · ", matches.values.len())
        } else {
            format!("{} · {} marked · ", matches.status(), editor.checked.len())
        };
        panel.row(
            out,
            panel.top + 6,
            &picker_prefixed(&prefix, directory, w),
            PICKER_MUTED,
            false,
            false,
        )?;
        let visible = panel.height as usize - 12;
        let selected = matches.selected_index();
        let start = selected
            .saturating_sub(visible.saturating_sub(1))
            .min(matches.values.len().saturating_sub(visible));
        for (index, item) in matches.values.iter().enumerate().skip(start).take(visible) {
            let active = index == selected;
            let marked = editor
                .checked
                .iter()
                .any(|p| p == &crate::path_complete::resolve(&item.path));
            let prefix = format!(
                "{} {} ",
                if active { "❯" } else { " " },
                if app.picker_template && !marked {
                    "   "
                } else if marked {
                    "[✓]"
                } else {
                    "[ ]"
                }
            );
            let label = item
                .path
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or(&item.path);
            let name = if let Some(files) = &app.recursive_files {
                std::path::Path::new(&item.path)
                    .strip_prefix(&files.root)
                    .unwrap_or(std::path::Path::new(&item.path))
                    .to_string_lossy()
                    .into_owned()
            } else {
                format!("{label}{}", if item.is_dir { "/" } else { "" })
            };
            let text = picker_prefixed(&prefix, &name, w);
            panel.row(
                out,
                panel.top + 7 + (index - start) as u16,
                &text,
                if active {
                    PICKER_ACCENT
                } else if item.is_dir {
                    PICKER_DIR
                } else {
                    PICKER_FG
                },
                active,
                false,
            )?;
        }
        if matches.values.is_empty() {
            panel.row(
                out,
                panel.top + 7,
                "No matching files or directories",
                PICKER_MUTED,
                false,
                false,
            )?;
        }
        let selected_path = matches.selected().map(|m| m.path.as_str()).unwrap_or("");
        panel.divider(out, bottom - 4)?;
        selected_path_line(out, panel.left + 2, bottom - 3, w, selected_path)?;
        let status = app.notice.as_deref().or(editor.error.as_deref()).unwrap_or(
            if app.picker_lut_atlas.is_some() {
                "↑↓ choose CSV · Ctrl+Y copy path · Esc back"
            } else if app.picker_atlas {
                "↑↓ choose · Ctrl+L choose CSV · Ctrl+O configured LUT"
            } else if app.picker_template {
                "↑↓ choose template · Ctrl+Y copy path"
            } else if app.recursive_files.is_some() {
                "↑↓ / Tab select · Space mark · Ctrl+Space query space · Ctrl+Y copy"
            } else {
                "↑↓ choose · Tab complete · ←→ parent/child · Space mark"
            },
        );
        panel.row(
            out,
            bottom - 2,
            status,
            if app.notice.is_some() {
                PICKER_SEARCH
            } else if editor.error.is_some() {
                Color::Red
            } else {
                PICKER_MUTED
            },
            false,
            false,
        )?;
        panel.row(
            out,
            bottom - 1,
            if app.picker_lut_atlas.is_some() {
                "Enter load atlas + LUT · Esc back · Ctrl+Z zoxide · Ctrl+R fd"
            } else if app.picker_atlas {
                "Enter load atlas · Esc back · Ctrl+Z zoxide · Ctrl+R fd"
            } else if app.picker_template {
                "Enter load template · Esc back · Ctrl+Z zoxide · Ctrl+R fd"
            } else {
                "Enter load · Esc back · Ctrl+Z zoxide · Ctrl+R fd · Ctrl+Y copy"
            },
            PICKER_MUTED,
            false,
            false,
        )?;
    }
    out.flush()?;
    Ok(())
}

fn draw_picker_plain(out: &mut impl Write, app: &App) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    queue!(out, Clear(ClearType::All))?;
    let Some(editor) = &app.edit else {
        return Ok(());
    };
    let x = if cols >= 70 { 3 } else { 1 };
    if let Some(picker) = &app.directory_picker {
        let width = cols.saturating_sub(x + 1) as usize;
        if rows == 0 {
            return Ok(());
        }
        line(
            out,
            0,
            "Zoxide navigation · choose directory",
            cols,
            Color::White,
            x,
        )?;
        if rows > 1 {
            line(
                out,
                1,
                &format!(
                    "Filter: {}  ·  {}/{} directories  ·  {} marked images",
                    filter_preview(&editor.text, app.picker_cursor, width.saturating_sub(8)),
                    picker.len(),
                    picker.total(),
                    editor.checked.len()
                ),
                cols,
                Color::Yellow,
                x,
            )?;
        }
        // Give most of the screen to ranked paths; reserve the lower section
        // for the selected directory's contents (like zi's preview).
        let available = (rows as usize).saturating_sub(6);
        let preview_rows = available / 3;
        let list_rows = available - preview_rows;
        let start = picker
            .selected
            .saturating_sub(list_rows.saturating_sub(1))
            .min(picker.len().saturating_sub(list_rows));
        for offset in 0..list_rows {
            let index = start + offset;
            let Some(dir) = picker.get(index) else {
                break;
            };
            let active = index == picker.selected;
            let prefix = format!("{} {:>7.1}  ", if active { ">" } else { " " }, dir.score);
            let path = safe_terminal_text(&dir.path.to_string_lossy());
            let txt = format!(
                "{prefix}{}",
                middle_ellipsis(&path, width.saturating_sub(prefix.chars().count()))
            );
            line(
                out,
                2 + offset as u16,
                &txt,
                cols,
                if active { Color::Cyan } else { Color::Grey },
                x,
            )?;
        }
        if rows > 2 && list_rows > 0 && picker.len() == 0 {
            let msg = picker
                .source_error
                .as_deref()
                .unwrap_or("No matching directories");
            line(out, 2, &safe_terminal_text(msg), cols, Color::DarkGrey, x)?;
        }
        if rows >= 7 {
            let divider = 2 + list_rows as u16;
            let path = picker
                .selected_dir()
                .map(|d| safe_terminal_text(&d.path.to_string_lossy()))
                .unwrap_or_default();
            let heading = format!("Contents · {}", path);
            line(
                out,
                divider,
                &middle_ellipsis(&heading, width),
                cols,
                Color::White,
                x,
            )?;
            if let Some(err) = &picker.preview_error {
                if preview_rows > 0 {
                    line(
                        out,
                        divider + 1,
                        &safe_terminal_text(err),
                        cols,
                        Color::Yellow,
                        x,
                    )?;
                }
            } else if preview_rows > 0
                && picker.preview.is_empty()
                && picker.selected_dir().is_some()
            {
                line(
                    out,
                    divider + 1,
                    "(No visible entries)",
                    cols,
                    Color::DarkGrey,
                    x,
                )?;
            } else {
                let first = picker
                    .preview_offset
                    .min(picker.preview.len().saturating_sub(preview_rows));
                for (idx, entry) in picker
                    .preview
                    .iter()
                    .skip(first)
                    .take(preview_rows)
                    .enumerate()
                {
                    let name = safe_terminal_text(&entry.name);
                    let label = format!("  {}{}", name, if entry.is_dir { "/" } else { "" });
                    line(
                        out,
                        divider + 1 + idx as u16,
                        &middle_ellipsis(&label, width),
                        cols,
                        if entry.is_dir {
                            Color::Cyan
                        } else {
                            Color::Grey
                        },
                        x,
                    )?;
                }
            }
            let end = divider + 1 + preview_rows as u16;
            let first = picker
                .preview_offset
                .min(picker.preview.len().saturating_sub(preview_rows));
            let last = (first + preview_rows).min(picker.preview.len());
            let status = format!(
                "{}–{} of {}{} entries · Ctrl+↑↓ scroll preview",
                if last == 0 { 0 } else { first + 1 },
                last,
                picker.preview.len(),
                if picker.preview_truncated { "+" } else { "" }
            );
            line(out, end, &status, cols, Color::DarkGrey, x)?;
        }
        if rows >= 2 {
            if let Some(e) = &editor.error {
                line(out, rows - 2, e, cols, Color::Red, x)?;
            } else {
                line(
                    out,
                    rows - 2,
                    "↑↓ / Tab select · PgUp/PgDn page · Enter jump",
                    cols,
                    Color::DarkGrey,
                    x,
                )?;
            }
        }
        if rows >= 1 {
            line(
                out,
                rows - 1,
                "Esc / Ctrl+Z back · Ctrl+Y copy path · Ctrl+C quit",
                cols,
                Color::DarkGrey,
                x,
            )?;
        }
        out.flush()?;
        return Ok(());
    }
    let Some(matches) = &editor.completion else {
        return Ok(());
    };
    let width = cols.saturating_sub(x + 1) as usize;
    if rows == 0 {
        return Ok(());
    }
    line(
        out,
        0,
        if app.picker_lut_atlas.is_some() {
            "Choose atlas LUT CSV"
        } else if app.picker_atlas {
            "Load solid label atlas"
        } else if app.picker_template {
            "Load anatomical template"
        } else {
            "Load NIfTI images"
        },
        cols,
        Color::White,
        x,
    )?;
    if rows > 1 {
        let filter = if editor.text.is_empty() {
            "▏(type any substring)".into()
        } else {
            filter_preview(&editor.text, app.picker_cursor, width.saturating_sub(8))
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
            &if app.picker_lut_atlas.is_some() {
                format!("{} CSV matches · in {}", matches.values.len(), directory)
            } else if app.picker_atlas {
                format!(
                    "{} · LUT: {} · in {}",
                    matches.values.len(),
                    if app.picker_atlas_lut {
                        "configured"
                    } else {
                        "auto / IDs"
                    },
                    directory
                )
            } else if app.picker_template {
                format!(
                    "{} matches · bottom layer · in {}",
                    matches.values.len(),
                    directory
                )
            } else {
                format!(
                    "{} · marked {} · in {}",
                    matches.status(),
                    editor.checked.len(),
                    directory
                )
            },
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
            if app.picker_template && !marked {
                "   "
            } else if marked {
                "[x]"
            } else {
                "[ ]"
            }
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
        selected_path_line(out, x, row, width, selected_path)?;
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
                if app.picker_template || app.picker_atlas {
                    "↑↓ choose · Tab complete · ←→ parent/child"
                } else {
                    "↑↓ choose · Tab complete · ←→ parent/child · Space mark"
                },
                cols,
                Color::DarkGrey,
                x,
            )?;
        }
        if app.notice.is_none() {
            line(
                out,
                rows - 2,
                if app.picker_lut_atlas.is_some() {
                    "Enter atlas + LUT · Esc back · Ctrl+Z zoxide · Ctrl+R fd"
                } else if app.picker_atlas {
                    "Enter atlas · Ctrl+L CSV · Ctrl+O configured LUT · Ctrl+Z zoxide"
                } else if app.picker_template {
                    "Enter template · Esc cancel · Ctrl+Z zoxide · Ctrl+R fd"
                } else {
                    "Enter load · Esc cancel · Ctrl+Z zoxide browser · PgUp/PgDn page"
                },
                cols,
                Color::DarkGrey,
                x,
            )?;
        }
    }
    out.flush()?;
    Ok(())
}

fn is_panel_heading(s: &str) -> bool {
    matches!(
        s,
        "NAVIGATION"
            | "LAYERS"
            | "VIEW"
            | "SELECTED IMAGE"
            | "ATLAS"
            | "REGION AT PROBE"
            | "IMAGE STACK"
            | "DISPLAY SETTINGS"
            | "ATLAS & PROJECTION"
            | "ATLAS & APPEARANCE"
            | "PROJECTION NOTES"
            | "ANNOTATIONS & APPEARANCE"
    ) || s.starts_with("MONTAGE ·")
        || s.starts_with("PROJECTION ·")
}
fn wrap_panel_line(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for ch in safe_terminal_text(text).chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        if w <= width {
            line.push(ch);
            used += w;
        }
    }
    lines.push(line);
    lines
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
        Some(Popup::Help) if app.projection != app::ProjectionMode::Slice => {
            lines.extend(
                [
                    "PROJECTION · full depth, per image",
                    "  X / A / U  Set selected layer MAX / AVG / MIN+",
                    "  Esc / V   Return to normal slices",
                    "  Other images default to MAX; choices follow image IDs.",
                    "",
                    "LAYERS",
                    "  1–9 / ;  Select image; Tab opens ordered layer list",
                    "  In layer list: ↑↓ / jk select; A/U set method; Enter closes",
                    "  Space     Hide/show selected image",
                    "  In layer list: Y copy · X cut · p/P paste below/above",
                    "  d unload · w raise · { lower · Y yank",
                    "",
                    "VIEW",
                    "  x c z     Change axis; repeat any axis key to mirror",
                    "  e/m/M/0  Edit/reset window; +/- contrast; ,/. brightness",
                    "  l palette · o opacity",
                    "",
                    "ATLAS & APPEARANCE",
                    "  a load/toggle atlas (pick if missing) · O atlas opacity",
                    "  Ctrl+A  Pick atlas (Ctrl+Z zoxide / Ctrl+R fd; Ctrl+L choose CSV)",
                    "  t / Ctrl+T  Load/reveal template; pick if default missing",
                    "  T enable/cycle major / broad / silhouette; a hide/show",
                    "  g edge AA · G atlas blur · b background · f fill",
                    "  n load images · Z zoxide · F fd · i info · p montage",
                    "  Q pixel budget · Shift+↑↓ reorder in layer list",
                    "",
                    "PROJECTION NOTES",
                    "  AVG includes zeros; nonfinite values are ignored.",
                    "  MAX favors high IDs; MIN+ favors low positive IDs.",
                    "  Overlapping labels cannot all occupy the same pixel.",
                    "  Projections cached on demand; choices not saved in scenes.",
                    "  q / Esc close help · Ctrl+C quit; Esc never quits",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        Some(Popup::Help) => {
            lines.extend(
                [
                    "NAVIGATION",
                    "  ↑↓←→ / jk      Slices; wheel / trackpad also scroll",
                    "  Shift ×4 · Ctrl ×3 · Alt ×2 · Cmd ×1",
                    "  PgUp / PgDn      ±25 slices    x c z   choose axis; repeat axis key mirrors",
                    "  [ / ] stride   r / R rotate   h / v flip   f fill",
                    "  I      Keyboard probe (arrows move probe; PgUp/Dn slices)",
                    "",
                    "  Ctrl+E Edit loaded image · p montage (press ? for controls)",
                    "",
                    "LAYERS",
                    "  1–9    Select layer      ;       Next layer",
                    "  n file picker · Z zoxide · F recursive fd (outside montage)",
                    "  In picker: Ctrl+Z zoxide · Ctrl+R fd · Ctrl+Y copy path",
                    "  s      Save current scene (.niv.toml)",
                    "         ↑↓ select · Tab complete · Space mark · Enter load",
                    "  d unload · w raise · { lower · Y yank path",
                    "  Reload saved scene: niv --scene FILE.niv.toml",
                    "         Atlas always stays on top; Tab shows stack order",
                    "  Space  Toggle selected image",
                    "  In layer list: X cut (move); p/P paste below/above; Y copy",
                    "",
                    "DISPLAY SETTINGS",
                    "  e      Edit min,max     m / M   Edit just min / max",
                    "  0      Reset range      l       Cycle color map",
                    "  o      Image opacity    O       Atlas opacity",
                    "",
                    "ATLAS & PROJECTION",
                    "  a      Load/toggle atlas (pick if missing) · X/A/U projections",
                    "  Ctrl+A  Pick atlas (Ctrl+Z zoxide / Ctrl+R fd; Ctrl+L choose CSV)",
                    "  t / Ctrl+T  Load/reveal template; pick if default missing",
                    "  g      Edge AA (optional)    V   Return to slices",
                    "  u or left click: copy hovered atlas region; OSC 52",
                    "  G      Atlas Gaussian blur (0 = off)",
                    "  b      Black / transparent",
                    "  Tab    Layer list: Tab / ↑↓ / jk select; Shift+↑↓ reorder",
                    "         Enter / q / Esc close; f fill; Q pixel budget",
                    "  + / -  Contrast         , / .   Brightness",
                    "  In filename prompts q is typed; Esc cancels",
                    "",
                    "? / help · i info · q close/quit · Ctrl+C quit",
                    "Esc closes menus or exits projection; never quits the viewer",
                    "Shortcuts shown are defaults; optional NIV_KEYS remaps viewer keys.",
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
                    "  ↑↓ / jk / wheel  Shift every slice (modifiers change stride)",
                    "  ←→  Adjust tile gap; PgUp/PgDn page within range",
                    "  Home/End first/last page · Hover or click to inspect",
                    "  H   Toggle delayed outline highlight (slower) · u copy region",
                    "",
                    "ANNOTATIONS & APPEARANCE",
                    "  N   Toggle slice numbers (off by default)",
                    "  C   Toggle IDs for a visible categorical layer",
                    "  f     Image only (no UI text) · Q montage pixel budget",
                    "  g   Edge AA    G   Atlas Gaussian blur (0..4 px)",
                    "  a   Load/toggle atlas    O   Atlas opacity",
                    "  Ctrl+A  Pick atlas (Ctrl+Z zoxide / Ctrl+R fd; Ctrl+L choose CSV)",
                    "  t / Ctrl+T  Load/reveal template; pick if default missing",
                    "",
                    "LAYERS",
                    "  1–9 select layer · e window · l palette · o opacity",
                    "  Projection toggles are disabled in montage mode",
                    "  n   Load images    Tab Layer list    s   Save scene",
                    "  In layer list: Tab / ↑↓ select · Shift+↑↓ reorder · Enter close",
                    "  In picker: Ctrl+Z zoxide · Ctrl+R fd · Ctrl+Y copy path",
                    "  In layer list: Y copy · X cut · p/P paste · d unload",
                    "  i   Info   q / ? / Esc close help · Ctrl+C quit",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        Some(Popup::Info) => {
            let id = app.selected_layer_id();
            let (source, range) = if id == 0 {
                (vol, base_range)
            } else {
                (
                    &layers.overlays[id - 1].vol,
                    layers.overlays[id - 1].data_range,
                )
            };
            let (lo, hi) = app.selected_window(layers);
            if layers.atlas.is_some() {
                lines.push("REGION AT PROBE".into());
                if app.projection != app::ProjectionMode::Slice {
                    lines.push("No unique 3D atlas region in a full-depth projection".into());
                } else if let Some(slice) = app.probe_index() {
                    let (w, h, _) = vol.plane(app.axis);
                    let (vh, vv) = app.probe.unwrap_or((w / 2, h / 2));
                    let (vh, vv) = (vh.min(w - 1), vv.min(h - 1));
                    let [x, y, z] = app.axis.xyz(slice, vh, vv);
                    lines.push(format!("Voxel      [{x},{y},{z}]"));
                    if let Some((label, _)) = layers.region_details(app.axis, slice, vh, vv) {
                        lines.push(format!("Region     {label}"));
                        lines.push(format!(
                            "Region ID  {}",
                            layers.atlas_id_at(app.axis, slice, vh, vv).unwrap_or(0)
                        ));
                    } else {
                        lines.push("Region ID  unavailable without solid atlas labels".into());
                    }
                } else {
                    lines.push("Hover or click a montage tile first".into());
                }
                lines.push(String::new());
            }
            lines.push("SELECTED IMAGE".into());
            lines.push(format!(
                "Selected image {}: {}",
                app.layer_number(app.selected),
                selected_name(app, layers)
            ));
            lines.push(format!(
                "Grid       {} × {} × {} voxels",
                source.dims()[0],
                source.dims()[1],
                source.dims()[2]
            ));
            lines.push(format!(
                "Spacing    {:.4} × {:.4} × {:.4}",
                source.spacing[0], source.spacing[1], source.spacing[2]
            ));
            lines.push(format!(
                "Stored     {} (NIfTI code {}) · decoded f32",
                source.datatype_name(),
                source.datatype
            ));
            lines.push(format!(
                "Data       {} .. {}",
                compact_number(range.0),
                compact_number(range.1)
            ));
            lines.push(format!(
                "Window     {} .. {}",
                compact_number(lo),
                compact_number(hi)
            ));
            lines.push(format!(
                "Opacity    {:.0}%",
                app.selected_opacity(layers) * 100.0
            ));
            lines.push(String::new());
            lines.push("VIEW".into());
            if app.montage.enabled {
                lines.push(format!(
                    "Axis       {} · rotation {}° · stride {}",
                    app.axis.label(),
                    app.rotation,
                    app.stride
                ));
            } else {
                lines.push(format!(
                    "Axis       {} slice {}/{}  · rotation {}° · stride {}",
                    app.axis.label(),
                    app.index + 1,
                    vol.depth(app.axis),
                    app.rotation,
                    app.stride
                ));
            }
            lines.push(format!("Projection {}", app.projection_for(id).label()));
            lines.push(format!(
                "Background {}",
                if app.transparent_background {
                    "RGBA"
                } else {
                    "RGB"
                }
            ));
            if app.montage.enabled {
                let view = layout(vol, app, layers, cols, rows);
                let reserved = (view.head + rows.saturating_sub(view.footer)) as usize;
                let plan = montage::plan(vol, app, cols, rows, reserved);
                let gap = montage::gap_micrometres(vol, app, &plan)
                    .map(|um| format!("{} px (≈{um:.0} µm)", plan.gap))
                    .unwrap_or_else(|| format!("{} px", plan.gap));
                lines.push(format!(
                    "Montage   {}..{} every {} | {} cols | gap {gap}",
                    app.montage.start, app.montage.stop, app.montage.interval, app.montage.columns
                ));
            }
            if let Some(atlas) = &layers.atlas {
                lines.push(String::new());
                lines.push("ATLAS".into());
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
            lines.push("IMAGE STACK".into());
            lines.push("Top to bottom · atlas above images".into());
            for pos in (0..=layers.overlays.len()).rev() {
                let id = app.layer_at(pos);
                if id == 0 {
                    lines.push(format!(
                        "  {}{} {}  {}  data {}..{}",
                        app.layer_number(pos),
                        projection_tag(app, id),
                        app.path,
                        vol.datatype_name(),
                        compact_number(base_range.0),
                        compact_number(base_range.1)
                    ));
                } else {
                    let layer = &layers.overlays[id - 1];
                    lines.push(format!(
                        "  {}{} {}  {}  data {}..{}",
                        app.layer_number(pos),
                        projection_tag(app, id),
                        layer.name,
                        layer.vol.datatype_name(),
                        compact_number(layer.data_range.0),
                        compact_number(layer.data_range.1)
                    ));
                }
            }
            lines.push(String::new());
            lines.push("q / Esc / i / ?  close · Ctrl+C quit".into());
        }
        None => return Ok(()),
    }
    if cols < 16 || rows < 8 {
        if rows > 0 {
            line(
                out,
                0,
                "Enlarge terminal · Esc close",
                cols,
                PICKER_MUTED,
                0,
            )?;
        }
        out.flush()?;
        return Ok(());
    }
    let panel = PickerPanel::new(cols, rows);
    panel.frame(out)?;
    let title = match app.popup {
        Some(Popup::Help) if app.projection != app::ProjectionMode::Slice => "Projection help",
        Some(Popup::Help) => "Viewer help",
        Some(Popup::MontageHelp) => "Montage help",
        _ => "Image information",
    };
    panel.title(out, title)?;
    panel.divider(out, panel.top + 2)?;
    // Wrap long names/paths and help text instead of silently clipping them.
    let lines: Vec<String> = lines
        .iter()
        .flat_map(|s| wrap_panel_line(s, panel.inner()))
        .collect();
    let max_lines = (panel.height as usize).saturating_sub(6);
    let showing = lines.len().min(max_lines);
    let first = app.popup_scroll.min(lines.len().saturating_sub(showing));
    for (offset, shown) in lines.iter().skip(first).take(showing).enumerate() {
        let heading = is_panel_heading(shown);
        if heading {
            queue!(out, SetAttribute(Attribute::Bold))?;
        }
        panel.row(
            out,
            panel.top + 3 + offset as u16,
            shown,
            if heading { PICKER_ACCENT } else { PICKER_FG },
            false,
            false,
        )?;
        if heading {
            queue!(out, SetAttribute(Attribute::Reset))?;
        }
    }
    panel.divider(out, panel.bottom() - 2)?;
    panel.row(
        out,
        panel.bottom() - 1,
        &format!(
            "{}–{} / {} · ↑↓ scroll · q/Esc close",
            if showing == 0 { 0 } else { first + 1 },
            first + showing,
            lines.len()
        ),
        PICKER_MUTED,
        false,
        false,
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
    if cols < 16 || rows < 8 {
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
            y_offset: 0,
            footer: 0,
            head: 0,
            list_start: 0,
            list_count: 0,
        });
    }
    let view = layout(vol, app, layers, cols, rows);
    let clear_text =
        frames.active_id.is_none() || previous.is_some_and(|old| layout_changed(old, view));
    let t = Instant::now();
    let pixels = if app.montage.enabled {
        let reserved = (view.head + rows.saturating_sub(view.footer)) as usize;
        let plan = montage::plan(vol, app, cols, rows, reserved);
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
    if clear_text {
        // Clear text cells only: the previous Kitty placement stays live until
        // replacement transmission completes. Never delete it before rasterizing.
        for row in 0..rows {
            queue!(out, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
        }
    }
    queue!(out, MoveTo(view.x, view.y))?;
    let next = frames.allocate();
    kitty::replace(
        out,
        next,
        &frame,
        view.geom,
        view.y_offset,
        frames.active_id,
    )?;
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

fn wheel_step(kind: MouseEventKind, mods: KeyModifiers) -> Option<(isize, usize)> {
    let (direction, horizontal) = match kind {
        MouseEventKind::ScrollUp => (1, false),
        MouseEventKind::ScrollDown => (-1, false),
        MouseEventKind::ScrollLeft => (1, true),
        MouseEventKind::ScrollRight => (-1, true),
        _ => return None,
    };
    // Some terminals convert Shift+wheel to horizontal events without
    // preserving the SHIFT bit. Treat unmodified horizontal input as Shift.
    let multiplier = if horizontal && mods.is_empty() {
        4
    } else {
        App::multiplier(mods, false)
    };
    Some((direction, multiplier))
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
    let reserved = (view.head + rows.saturating_sub(view.footer)) as usize;
    let plan = montage::plan(vol, app, cols, rows, reserved);
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
    let box_h = (geom.rows as usize * ch).saturating_sub(view.y_offset) as f64;
    if box_h <= 0. {
        return None;
    }
    let scale = (box_w / geom.width as f64).min(box_h / geom.height as f64);
    let display_w = geom.width as f64 * scale;
    let display_h = geom.height as f64 * scale;
    let x = mx - (box_w - display_w) * 0.5;
    let y = my - view.y_offset as f64 - (box_h - display_h) * 0.5;
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
/// swallowed. The wheel path separately coalesces its own queued events; probe movement remains precise.
fn is_slice_key(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind};
    key.kind != KeyEventKind::Release
        && matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Char('j' | 'k' | 'H' | 'J' | 'K')
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

/// Collapse queued like-direction wheel events into one update. Mouse motion
/// interleaved with scrolls is superseded by the last wheel position. Do not
/// swallow clicks, keystrokes, resizes or changes of wheel direction/modifier.
fn same_wheel(a: MouseEvent, b: MouseEvent) -> bool {
    wheel_step(a.kind, a.modifiers).is_some() && a.kind == b.kind && a.modifiers == b.modifiers
}
fn coalesce_wheel(mut latest: MouseEvent, next_event: &mut Option<Event>) -> Result<MouseEvent> {
    let mut trailing_motion = None;
    for _ in 0..4096 {
        if !event::poll(Duration::ZERO)? {
            break;
        }
        match event::read()? {
            Event::Mouse(m) if same_wheel(latest, m) => {
                latest = m;
                trailing_motion = None;
            }
            Event::Mouse(m) if m.kind == MouseEventKind::Moved => {
                trailing_motion = Some(m);
            }
            other => {
                *next_event = Some(other);
                break;
            }
        }
    }
    if next_event.is_none() {
        *next_event = trailing_motion.map(Event::Mouse);
    }
    Ok(latest)
}

fn set_montage_probe(app: &mut App, probe: Option<(usize, (usize, usize))>) -> bool {
    let (slice, hv) = match probe {
        Some((s, hv)) => (Some(s), Some(hv)),
        None => (None, None),
    };
    let changed = app.montage_probe_slice != slice || app.probe != hv;
    app.montage_probe_slice = slice;
    app.probe = hv;
    changed
}
fn hover_target(app: &App, layers: &Layers, delay: Duration) -> Option<(Option<usize>, i32)> {
    if delay.is_zero()
        || app.projection != app::ProjectionMode::Slice
        || app.inspect_mode
        || !app.show_atlas
        || app.edit.is_some()
        || app.popup.is_some()
        || (app.montage.enabled && !app.montage_hover_highlight)
    {
        return None;
    }
    let slice = app.probe_index()?;
    let (h, v) = app.probe?;
    layers
        .atlas_id_at(app.axis, slice, h, v)
        .map(|id| (app.montage.enabled.then_some(slice), id))
}
fn schedule_hover(
    app: &App,
    layers: &Layers,
    delay: Duration,
    candidate: &mut Option<(Option<usize>, i32)>,
    deadline: &mut Option<Instant>,
) {
    let target = hover_target(app, layers, delay);
    let current = app
        .highlighted_region
        .map(|id| (app.montage_highlight_slice, id));
    if target != *candidate || (target != current && deadline.is_none()) {
        *candidate = target;
        *deadline = Some(Instant::now() + delay);
    }
}

/// OSC 52 requests clipboard access at the viewing terminal (including SSH).
fn copy_region(out: &mut impl Write, vol: &Volume, app: &App, layers: &Layers) -> Result<bool> {
    if app.projection != app::ProjectionMode::Slice {
        return Ok(false);
    }
    let Some(slice) = app.probe_index() else {
        return Ok(false);
    };
    let (w, h, _) = vol.plane(app.axis);
    let (u, v) = app.probe.unwrap_or((w / 2, h / 2));
    let Some(id) = layers.atlas_id_at(app.axis, slice, u, v) else {
        return Ok(false);
    };
    let Some(info) = layers.atlas.as_ref().and_then(|a| a.regions.get(&id)) else {
        return Ok(false);
    };
    use base64::Engine;
    let text = format!("{} — {}", info.abbr, info.name);
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // Prefer OSC 52 for remote sessions; no shell subprocess needed.
    write!(out, "\x1b]52;c;{}\x07", encoded)?;
    out.flush()?;
    Ok(true)
}

/// Copy the settings, not the voxel array. Paste reloads the file only on demand.
#[derive(Clone)]
struct YankedImage {
    path: std::path::PathBuf,
    window: (f32, f32),
    opacity: f32,
    palette: layers::Palette,
    visible: bool,
    projection: app::ProjectionMode,
}
impl YankedImage {
    fn capture(config: &config::Config, app: &App, layers: &Layers) -> Self {
        let id = app.selected_layer_id();
        let source = if id == 0 {
            &config.path
        } else {
            &layers.overlays[id - 1].path
        };
        Self {
            path: source.clone(),
            window: app.selected_window(layers),
            opacity: app.selected_opacity(layers),
            palette: app.selected_palette(layers),
            visible: if id == 0 {
                app.base_visible
            } else {
                layers.overlays[id - 1].visible
            },
            projection: app
                .layer_projections
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, m)| *m)
                .unwrap_or(app::ProjectionMode::Max),
        }
    }
    fn paste(&self, app: &mut App, layers: &mut Layers, vol: &Volume, above: bool) -> Result<()> {
        let id = layers.add_overlay(&self.path, vol)?;
        let copy = &mut layers.overlays[id - 1];
        copy.window = self.window;
        copy.opacity = self.opacity;
        copy.palette = self.palette;
        copy.visible = self.visible;
        let pos = app.selected + usize::from(above);
        app.layer_order.insert(pos, id);
        app.selected = pos;
        app.layer_projections.push((id, self.projection));
        Ok(())
    }
}

/// Remove a layer, promoting an existing aligned image if ID zero is removed.
/// All ID-indexed settings and cached planes must follow the same remapping.
fn unload_selected(
    vol: &mut Volume,
    base_range: &mut (f32, f32),
    config: &mut config::Config,
    app: &mut App,
    layers: &mut Layers,
) {
    if layers.overlays.is_empty() {
        return;
    }
    let removed = app.selected_layer_id();
    let last = layers.overlays.len();
    let remap = |id: usize| -> Option<usize> {
        if id == removed {
            None
        } else if removed == 0 {
            Some(if id == last { 0 } else { id })
        } else {
            Some(if id > removed { id - 1 } else { id })
        }
    };
    if removed == 0 {
        let promoted = layers.overlays.pop().expect("another image exists");
        *vol = promoted.vol;
        *base_range = promoted.data_range;
        config.path = promoted.path;
        app.path = promoted.name;
        app.window = promoted.window;
        app.default_window = promoted.default_window;
        app.base_palette = promoted.palette;
        app.base_opacity = promoted.opacity;
        app.base_visible = promoted.visible && app.show_overlays;
    } else {
        layers.overlays.remove(removed - 1);
    }
    app.layer_order = app.layer_order.iter().filter_map(|&id| remap(id)).collect();
    app.layer_projections = app
        .layer_projections
        .iter()
        .filter_map(|&(id, mode)| remap(id).map(|i| (i, mode)))
        .collect();
    app.cut_layer = app.cut_layer.and_then(remap);
    app.projection_cache.remap_images(remap);
    app.selected = app.selected.min(app.layer_order.len() - 1);
}

fn env_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Load only the requested atlas. Commit state after validation; on failure
/// keep the existing atlas, paths and cached projections intact.
fn replace_atlas(
    path: &std::path::Path,
    lut: Option<std::path::PathBuf>,
    vol: &Volume,
    config: &mut config::Config,
    app: &mut App,
    layers: &mut Layers,
) -> Result<()> {
    let atlas = layers::load_atlas(
        path,
        vol,
        false,
        None,
        lut.as_deref(),
        config.atlas_color,
        config.atlas_width,
        config.atlas_opacity,
    )?;
    config.atlas_path = Some(path.to_path_buf());
    config.atlas_wireframe = false;
    config.atlas_labels_path = None;
    config.lut_path = lut;
    app.atlas_name = Some(layers::short_name(path));
    layers.atlas = Some(atlas);
    app.projection_cache.clear_atlas();
    app.highlighted_region = None;
    app.montage_highlight_slice = None;
    app.show_atlas = true;
    if app.projection != app::ProjectionMode::Slice {
        app.show_projected_atlas = true;
    }
    Ok(())
}

/// Reuse an already loaded template rather than decoding it a second time.
/// Keep its display settings and the current selection; place it at the bottom.
fn reveal_template(
    path: &std::path::Path,
    vol: &Volume,
    base_path: &std::path::Path,
    app: &mut App,
    layers: &mut Layers,
) -> Result<()> {
    let selected = app.selected_layer_id();
    let id = if same_file(path, base_path) {
        0
    } else if let Some(index) = layers
        .overlays
        .iter()
        .position(|l| same_file(path, &l.path))
    {
        index + 1
    } else {
        let id = layers.add_overlay(path, vol)?;
        layers.overlays[id - 1].palette = layers::Palette::Gray;
        app.layer_projections.push((id, app::ProjectionMode::Mean));
        id
    };
    app.sync_layer_order(layers.overlays.len());
    app.layer_order.retain(|&i| i != id);
    app.layer_order.insert(0, id);
    app.selected = app
        .layer_order
        .iter()
        .position(|&i| i == selected)
        .unwrap_or(0);
    if id == 0 {
        app.base_visible = true;
        if app.base_opacity == 0. {
            app.base_opacity = 1.;
        }
    } else {
        app.show_overlays = true;
        let layer = &mut layers.overlays[id - 1];
        layer.visible = true;
        if layer.opacity == 0. {
            layer.opacity = 1.;
        }
    }
    Ok(())
}

// Successful file drops need no status row; retain useful partial-failure details.
fn paste_load_notice(loaded: usize, errors: &[String]) -> Option<String> {
    if errors.is_empty() {
        None
    } else {
        Some(format!(
            "Loaded {loaded}; {} failed: {}",
            errors.len(),
            errors[0]
        ))
    }
}

/// Decode literal or shell-escaped paths from a terminal paste, never execute them.
fn pasted_image_paths(text: &str) -> Result<Vec<std::path::PathBuf>> {
    let text = text.trim();
    let literal = path_complete::resolve(text);
    if path_complete::is_nifti_file(&literal) {
        return Ok(vec![literal]);
    }
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            (None, '\\') | (Some('"'), '\\') => {
                let next = chars
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Incomplete path escape"))?;
                word.push(next);
            }
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(c),
        }
    }
    if quote.is_some() {
        bail!("Unclosed path quote");
    }
    if !word.is_empty() {
        words.push(word);
    }
    let paths = words
        .iter()
        .map(|w| path_complete::resolve(w))
        .collect::<Vec<_>>();
    if paths.is_empty() || paths.iter().any(|p| !path_complete::is_nifti_file(p)) {
        bail!("Paste one or more existing .nii/.nii.gz paths on this host");
    }
    Ok(paths)
}

fn run() -> Result<()> {
    if edit::maybe_run()? {
        return Ok(());
    }
    let Some(mut config) = config::parse()? else {
        return Ok(());
    };
    let mut vol = Volume::load(&config.path)?;
    let index = config.initial_slice.unwrap_or(vol.depth(config.axis) / 2);
    if index >= vol.depth(config.axis) {
        bail!(
            "--slice {index} is outside axis {} (0..{})",
            config.axis.label(),
            vol.depth(config.axis) - 1
        );
    }
    let mut base_range = vol.data_range();
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
    app.base_palette = layers::default_palette(&config.path);
    if config.atlas_only {
        app.base_visible = false;
        app.base_palette = layers::Palette::Random;
    }
    app.pixel_budget = config.pixel_budget;
    if let Ok(value) = std::env::var("NIV_MONTAGE_PIXEL_BUDGET") {
        app.montage_pixel_budget = value
            .parse::<usize>()
            .ok()
            .filter(|&n| n <= 10_000_000)
            .ok_or_else(|| anyhow::anyhow!("NIV_MONTAGE_PIXEL_BUDGET must be 0..10000000"))?;
    }
    app.atlas_aa = config.atlas_aa;
    app.atlas_blur = config.atlas_blur;
    app.base_opacity = config.base_opacity;
    app.transparent_background = config.transparent_background;
    app.reverse_key_scroll = config.reverse_key_scroll;
    app.default_window = vol.auto_window();
    app.atlas_name = config.atlas_path.as_ref().map(|p| layers::short_name(p));
    app.sync_layer_order(layers.overlays.len());
    app.layer_order.reverse();
    app.selected = app.layer_order.len() - 1;
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
    let keymap = keymap::Keymap::load()?;
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
    let mut yanked: Option<YankedImage> = None;
    let mut pending_frame = false;
    let mut pending_ui = false;
    let mut pending_layout = false;
    let mut deferred_event: Option<Event> = None;
    // The candidate region is updated immediately on hover; graphics are only
    // redrawn if the candidate differs from the active highlight after a delay.
    let mut hover_candidate: Option<(Option<usize>, i32)> = None;
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
                        let key = keymap.remap(&app, key);
                        // Edit an already-loaded NIfTI directly, without requiring a mask.
                        // Keep the viewer's terminal guard and restore its state afterwards.
                        if app.edit.is_none()
                            && app.popup.is_none()
                            && !app.show_layer_list
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                            && matches!(key.code, crossterm::event::KeyCode::Char('e' | 'E'))
                        {
                            frames.clear(&mut out)?;
                            out.flush()?;
                            let result = edit::run_from_viewer(
                                &config.path,
                                app.axis.index(),
                                app.index,
                                &mut out,
                            );
                            queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
                            result?;
                            app.notice=Some("Returned from segmentation editing; saved files can be loaded with n".into());
                            pending_layout = true;
                            pending_frame = true;
                            last_draw = Instant::now() - frame_interval;
                            break;
                        }

                        if app.notice.take().is_some() {
                            pending_layout = true;
                            pending_frame = true;
                        }
                        let before_index = app.index;
                        let before_montage = app.montage;
                        let before_transform = (
                            app.axis,
                            app.rotation,
                            app.flip_h,
                            app.flip_v,
                            app.fullscreen,
                            app.show_layer_list,
                        );
                        let action = app.key(&vol, &mut layers, key.clone());
                        navigated = before_index != app.index || before_montage != app.montage;
                        if app.montage.enabled
                            && (navigated
                                || before_transform
                                    != (
                                        app.axis,
                                        app.rotation,
                                        app.flip_h,
                                        app.flip_v,
                                        app.fullscreen,
                                        app.show_layer_list,
                                    ))
                        {
                            app.clear_montage_probe();
                            hover_candidate = None;
                            hover_deadline = None;
                        }
                        if matches!(action, Action::Redraw | Action::Layout) {
                            if !app.montage.enabled
                                && (app.projection != app::ProjectionMode::Slice
                                    || app.inspect_mode
                                    || !app.show_atlas)
                            {
                                app.highlighted_region = None;
                                app.montage_highlight_slice = None;
                            }
                            schedule_hover(
                                &app,
                                &layers,
                                hover_delay,
                                &mut hover_candidate,
                                &mut hover_deadline,
                            );
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
                            Action::CopyRegion => {
                                let copied = copy_region(&mut out, &vol, &app, &layers)?;
                                app.notice = Some(
                                    if copied {
                                        "Region clipboard copy requested"
                                    } else {
                                        "No named atlas region at cursor"
                                    }
                                    .into(),
                                );
                                pending_layout = true;
                                pending_frame = true;
                            }
                            Action::CopyPath(path) => {
                                use base64::Engine;
                                let encoded = base64::engine::general_purpose::STANDARD
                                    .encode(path.to_string_lossy().as_bytes());
                                write!(out, "\x1b]52;c;{}\x07", encoded)?;
                                out.flush()?;
                                if let Some(edit) = app.edit.as_mut() {
                                    edit.error = None;
                                }
                                app.notice = Some("Path clipboard copy requested".into());
                                pending_frame = true;
                            }
                            Action::YankLayer => {
                                let image = YankedImage::capture(&config, &app, &layers);
                                let cwd = std::env::current_dir().ok();
                                let path = cwd
                                    .as_deref()
                                    .and_then(|dir| image.path.strip_prefix(dir).ok())
                                    .unwrap_or(&image.path);
                                use base64::Engine;
                                let encoded = base64::engine::general_purpose::STANDARD
                                    .encode(path.to_string_lossy().as_bytes());
                                write!(out, "\x1b]52;c;{}\x07", encoded)?;
                                out.flush()?;
                                yanked = Some(image);
                                app.notice = Some("Image yanked · p/P in t to duplicate".into());
                                pending_layout = true;
                                pending_frame = true;
                            }
                            Action::PasteLayer(above) => {
                                if let Some(image) = &yanked {
                                    let notice =
                                        match image.paste(&mut app, &mut layers, &vol, above) {
                                            Ok(()) => "Image duplicated".into(),
                                            Err(err) => format!("Paste failed: {err:#}"),
                                        };
                                    app.notice = Some(notice);
                                } else {
                                    app.notice = Some("Nothing yanked; press Y first".into());
                                }
                                pending_layout = true;
                                pending_frame = true;
                            }
                            Action::UnloadLayer => {
                                unload_selected(
                                    &mut vol,
                                    &mut base_range,
                                    &mut config,
                                    &mut app,
                                    &mut layers,
                                );
                                pending_layout = true;
                                pending_frame = true;
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
                            Action::LoadAtlas(chosen) => {
                                let default = env_path("ATLAS");
                                let from_picker = chosen.is_some();
                                if !from_picker && !default.as_ref().is_some_and(|p| p.is_file()) {
                                    app.open_atlas_picker();
                                    app.notice = None;
                                } else {
                                    let use_lut = !from_picker
                                        || app.picker_atlas_lut
                                        || chosen
                                            .as_ref()
                                            .zip(default.as_ref())
                                            .is_some_and(|(a, b)| same_file(a, b));
                                    let lut = if from_picker && app.picker_lut_atlas.is_some() {
                                        app.picker_lut_path.clone()
                                    } else if use_lut {
                                        env_path("ATLAS_LUT")
                                    } else {
                                        None
                                    };
                                    let path = chosen.or(default);
                                    let result = if let Some(path) = path {
                                        app.notice =
                                            Some(format!("Loading atlas: {} ...", path.display()));
                                        if from_picker {
                                            draw_picker(&mut out, &app)?;
                                        }
                                        replace_atlas(
                                            &path,
                                            lut,
                                            &vol,
                                            &mut config,
                                            &mut app,
                                            &mut layers,
                                        )
                                    } else {
                                        Err(anyhow::anyhow!(
                                            "ATLAS is not set; Ctrl+A opens atlas picker"
                                        ))
                                    };
                                    match result {
                                        Ok(()) => {
                                            app.edit = None;
                                            app.recursive_files = None;
                                            app.directory_picker = None;
                                            app.picker_atlas = false;
                                            app.picker_lut_atlas = None;
                                            app.picker_lut_path = None;
                                            app.notice = Some("Atlas loaded".into());
                                            hover_candidate = None;
                                            hover_deadline = None;
                                        }
                                        Err(err) => {
                                            let msg = format!("Atlas load failed: {err:#}");
                                            if let Some(editor) = app.edit.as_mut() {
                                                editor.error = Some(msg);
                                                editor.checked.clear();
                                                app.notice = None;
                                            } else {
                                                app.notice = Some(msg);
                                            }
                                        }
                                    }
                                }
                                pending_layout = true;
                                pending_frame = true;
                            }
                            Action::LoadTemplate(chosen) => {
                                let from_picker = chosen.is_some();
                                let path =
                                    chosen.or_else(|| env_path("TEMPLATE").filter(|p| p.is_file()));
                                if let Some(path) = path {
                                    if from_picker {
                                        app.notice = Some(format!(
                                            "Loading template: {} ...",
                                            path.display()
                                        ));
                                        draw_picker(&mut out, &app)?;
                                    }
                                    match reveal_template(
                                        &path,
                                        &vol,
                                        &config.path,
                                        &mut app,
                                        &mut layers,
                                    ) {
                                        Ok(()) => {
                                            app.edit = None;
                                            app.recursive_files = None;
                                            app.directory_picker = None;
                                            app.picker_template = false;
                                            app.notice = None;
                                        }
                                        Err(err) => {
                                            let msg = format!("Template load failed: {err:#}");
                                            if let Some(editor) = app.edit.as_mut() {
                                                editor.error = Some(msg);
                                                editor.checked.clear();
                                                app.notice = None;
                                            } else {
                                                app.notice = Some(msg);
                                            }
                                        }
                                    }
                                } else {
                                    app.open_template_picker();
                                    app.notice = None;
                                }
                                pending_layout = true;
                                pending_frame = true;
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
                                app.recursive_files = None;
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
                    Event::Paste(text) => {
                        // Never interpret pasted text as viewer shortcuts. In ordinary
                        // viewing mode, a file drop can load aligned intensity layers.
                        if app.edit.is_none() && app.popup.is_none() {
                            app.notice = match pasted_image_paths(&text) {
                                Ok(paths) => {
                                    let mut loaded = 0;
                                    let mut errors = Vec::new();
                                    for path in paths {
                                        match layers.add_overlay(&path, &vol) {
                                            Ok(_) => loaded += 1,
                                            Err(err) => {
                                                errors.push(format!("{}: {err:#}", path.display()))
                                            }
                                        }
                                    }
                                    app.sync_layer_order(layers.overlays.len());
                                    if loaded > 0 {
                                        app.selected = app.layer_order.len() - 1;
                                    }
                                    paste_load_notice(loaded, &errors)
                                }
                                Err(err) => Some(format!("Paste: {err:#}")),
                            };
                            pending_layout = true;
                            pending_frame = true;
                        } else if let Some(editor) = app.edit.as_mut() {
                            editor.error =
                                Some("Close this editor before dropping image files".into());
                            pending_layout = true;
                            pending_frame = true;
                        }
                    }
                    Event::Resize(_, _) => {
                        if app.montage.enabled {
                            app.clear_montage_probe();
                            hover_candidate = None;
                            hover_deadline = None;
                        }
                        pending_layout = true;
                        pending_frame = true;
                        last_draw = Instant::now() - frame_interval;
                    }
                    Event::Mouse(mouse) => {
                        match mouse.kind {
                            MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight
                                if app.edit.is_none() && app.popup.is_none() =>
                            {
                                let mouse = coalesce_wheel(mouse, &mut deferred_event)?;
                                let (direction, multiplier) =
                                    wheel_step(mouse.kind, mouse.modifiers).unwrap();
                                let direction = if config.reverse_wheel_scroll {
                                    -direction
                                } else {
                                    direction
                                };
                                let delta = direction * (app.stride * multiplier) as isize;
                                let changed = if app.montage.enabled {
                                    app.shift_montage(&vol, delta)
                                } else {
                                    app.move_slices(&vol, delta)
                                };
                                if changed {
                                    pending_frame = true;
                                    navigated = true;
                                }
                                if app.montage.enabled {
                                    let probe = mouse_montage_tile(
                                        &vol,
                                        &app,
                                        view,
                                        mouse,
                                        config.mouse_pixels,
                                    );
                                    if set_montage_probe(&mut app, probe) {
                                        pending_ui = true;
                                    }
                                } else if !app.inspect_mode {
                                    let probe =
                                        mouse_voxel(&vol, &app, view, mouse, config.mouse_pixels);
                                    if app.probe != probe {
                                        app.probe = probe;
                                        pending_ui = true;
                                    }
                                }
                                // Dilation should follow the stable final slice,
                                // not introduce another expensive redraw during a burst.
                                schedule_hover(
                                    &app,
                                    &layers,
                                    hover_delay,
                                    &mut hover_candidate,
                                    &mut hover_deadline,
                                );
                                if changed && hover_candidate.is_some() {
                                    hover_deadline = Some(Instant::now() + hover_delay);
                                }
                            }
                            MouseEventKind::Moved
                            | MouseEventKind::Drag(_)
                            | MouseEventKind::Down(_)
                                if app.montage.enabled
                                    && app.edit.is_none()
                                    && app.popup.is_none() =>
                            {
                                let probe = mouse_montage_tile(
                                    &vol,
                                    &app,
                                    view,
                                    mouse,
                                    config.mouse_pixels,
                                );
                                if set_montage_probe(&mut app, probe) {
                                    pending_ui = true;
                                }
                                schedule_hover(
                                    &app,
                                    &layers,
                                    hover_delay,
                                    &mut hover_candidate,
                                    &mut hover_deadline,
                                );
                            }
                            MouseEventKind::Moved
                            | MouseEventKind::Drag(_)
                            | MouseEventKind::Down(_)
                                if !app.montage.enabled
                                    && !app.inspect_mode
                                    && app.edit.is_none()
                                    && app.popup.is_none() =>
                            {
                                let next =
                                    mouse_voxel(&vol, &app, view, mouse, config.mouse_pixels);
                                if app.probe != next {
                                    app.probe = next;
                                    pending_ui = true;
                                }
                                if next.is_some()
                                    && mouse.kind
                                        == MouseEventKind::Down(crossterm::event::MouseButton::Left)
                                    && app.projection == app::ProjectionMode::Slice
                                {
                                    let _ = copy_region(&mut out, &vol, &app, &layers)?;
                                }
                                schedule_hover(
                                    &app,
                                    &layers,
                                    hover_delay,
                                    &mut hover_candidate,
                                    &mut hover_deadline,
                                );
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                if navigated || (deferred_event.is_none() && !event::poll(Duration::ZERO)?) {
                    break;
                }
            }
        }
        if hover_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            hover_deadline = None;
            // Re-evaluate after possible menu, atlas or geometry changes.
            let target = hover_target(&app, &layers, hover_delay);
            let current = app
                .highlighted_region
                .map(|id| (app.montage_highlight_slice, id));
            if current != target {
                app.highlighted_region = target.map(|(_, id)| id);
                app.montage_highlight_slice = target.and_then(|(slice, _)| slice);
                pending_frame = true;
            }
            hover_candidate = target;
        }
        if (pending_frame || pending_ui || pending_layout) && last_draw.elapsed() >= frame_interval
        {
            // Account for raster+transfer time when pacing frames: the next
            // frame interval starts at draw BEGIN, not after slow I/O finishes.
            let draw_started = Instant::now();
            if pending_frame {
                let previous = Some(view);
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
    #[test]
    fn successful_file_drops_are_silent_but_errors_are_reported() {
        assert_eq!(paste_load_notice(2, &[]), None);
        let errors = vec!["bad.nii.gz: grid mismatch".to_string()];
        assert_eq!(
            paste_load_notice(1, &errors).as_deref(),
            Some("Loaded 1; 1 failed: bad.nii.gz: grid mismatch")
        );
    }

    #[test]
    fn pasted_paths_accept_spaces_and_multiple_files() {
        let dir = std::env::temp_dir().join(format!("niv-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("first image.nii.gz");
        let b = dir.join("second.nii");
        std::fs::write(&a, b"").unwrap();
        std::fs::write(&b, b"").unwrap();
        assert_eq!(
            pasted_image_paths(a.to_str().unwrap()).unwrap(),
            vec![a.clone()]
        );
        let escaped = a.to_str().unwrap().replace(' ', "\\ ");
        assert_eq!(
            pasted_image_paths(&format!("{} '{}'", escaped, b.display())).unwrap(),
            vec![a, b]
        );
        assert!(pasted_image_paths("not an image").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn mouse(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }
    #[test]
    fn info_numbers_keep_integer_and_fractional_values_distinct() {
        assert_eq!(intensity(20.), "20");
        assert_eq!(intensity(-0.), "0");
        assert_eq!(compact_number(1130.), "1130");
        assert_eq!(intensity(0.9999999), "0.9999999");
        assert_eq!(compact_number(0.99994), "0.99994");
        assert_eq!(compact_number(0.9999999), "0.9999999");
        assert_eq!(intensity(-1.25), "-1.2500000");
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
        assert_eq!(runs.len(), name.chars().count());
        assert!(runs.iter().all(|(_, s)| s.chars().count() == 1));
        assert!(runs.windows(2).all(|p| p[0].0 != p[1].0));
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
    fn wheel_bursts_only_coalesce_matching_direction_and_modifiers() {
        let a = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            ..mouse(1, 2)
        };
        let b = MouseEvent {
            column: 4,
            row: 5,
            ..a
        };
        assert!(same_wheel(a, b));
        assert!(!same_wheel(
            a,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..a
            }
        ));
        assert!(!same_wheel(
            a,
            MouseEvent {
                modifiers: KeyModifiers::SHIFT,
                ..a
            }
        ));
        assert!(!same_wheel(a, mouse(1, 2)));
    }
    #[test]
    fn wheel_modifiers_and_horizontal_fallback() {
        assert_eq!(
            wheel_step(MouseEventKind::ScrollUp, KeyModifiers::NONE),
            Some((1, 1))
        );
        assert_eq!(
            wheel_step(MouseEventKind::ScrollDown, KeyModifiers::SHIFT),
            Some((-1, 4))
        );
        assert_eq!(
            wheel_step(MouseEventKind::ScrollLeft, KeyModifiers::NONE),
            Some((1, 4))
        );
        assert_eq!(
            wheel_step(MouseEventKind::ScrollRight, KeyModifiers::CONTROL),
            Some((-1, 3))
        );
        assert_eq!(
            wheel_step(MouseEventKind::ScrollDown, KeyModifiers::SUPER),
            Some((-1, 1))
        );
        assert_eq!(
            wheel_step(MouseEventKind::ScrollUp, KeyModifiers::ALT),
            Some((1, 2))
        );
        assert_eq!(wheel_step(MouseEventKind::Moved, KeyModifiers::NONE), None);
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
            y_offset: 0,
            footer: 15,
            head: 2,
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
        let shifted = ViewLayout {
            y_offset: 8,
            geom: render::Geometry {
                height: 72,
                ..view.geom
            },
            ..view
        };
        assert_eq!(
            mouse_raster(shifted, mouse(36, 80), true, 8, 16),
            Some((20, 24))
        );
        assert_eq!(mouse_raster(shifted, mouse(36, 50), true, 8, 16), None);
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
    fn centered_header_never_overlaps_help_or_left_band() {
        for cols in [16u16, 32, 52, 80, 120, 160, 240] {
            let (hint, hint_x, center, first) = header_bands(cols, true);
            let center_x = (cols as usize - center) / 2;
            assert_eq!(hint_x + hint.len(), cols as usize);
            assert!(first < center_x);
            assert!(center_x + center < hint_x);
            let (_, hint_x, center, first) = header_bands(cols, false);
            assert_eq!(center, 0);
            assert_eq!(first + 1, hint_x);
        }
    }

    #[test]
    fn readout_wraps_into_gutter_or_dedicated_header_rows() {
        let parts = [
            "[139,133,190]",
            "| 1: 0",
            "| 2: 608",
            "| 3: 0.1234567",
            "| 4: 40",
        ]
        .map(str::to_owned);
        // Wide pane, no image shift: keep overflow in the left gutter.
        let widths = header_widths(31, 1, 140, 26, 30);
        assert_eq!(widths, vec![31, 25, 25, 25]);
        let (lines, used) = pack_header(&parts, &widths);
        assert_eq!(used, parts.len());
        assert_eq!(lines[0], "[139,133,190] | 1: 0 | 2: 608");
        assert_eq!(lines[1], "| 3: 0.1234567 | 4: 40");
        // Narrow pane: allocate one full row above the image instead.
        let narrow = header_widths(22, 2, 60, 0, 20);
        let (lines, used) = pack_header(&parts, &narrow);
        assert_eq!(used, parts.len());
        assert_eq!(lines[0], "[139,133,190] | 1: 0");
        assert_eq!(lines[1], "| 2: 608 | 3: 0.1234567 | 4: 40");
        assert!(lines[2..].iter().all(String::is_empty));
    }

    #[test]
    fn hover_does_not_change_layout_and_hidden_layers_leave_readout() {
        let vol = Volume {
            data: ndarray::Array3::zeros((8, 9, 10)),
            spacing: [1.; 3],
            spatial_units: 2,
            sform: None,
            datatype: 16,
        };
        let mut app = App::new(volume::Axis::X, 3, 1, 90, (0., 1000.), "test".into());
        let layers = Layers::default();
        let old = layout(&vol, &app, &layers, 120, 40);
        app.probe = Some((1, 2));
        assert!(!layout_changed(old, layout(&vol, &app, &layers, 120, 40)));
        assert_eq!(header_estimate(&vol, &app, &layers).len(), 2);
        app.base_visible = false;
        assert_eq!(header_estimate(&vol, &app, &layers).len(), 1);
    }

    #[test]
    fn half_cell_inset_reclaims_the_previously_reserved_row() {
        let vol = Volume {
            data: ndarray::Array3::zeros((10, 10, 10)),
            spacing: [1.; 3],
            spatial_units: 2,
            sform: None,
            datatype: 16,
        };
        let app = App::new(volume::Axis::X, 3, 1, 90, (0., 1.), "test".into());
        // 100 columns x 40 rows, 8x16 pixel cells; one header and three footer rows.
        let old = render::geometry_with_inset(&vol, &app, 100, 40, 5, (8, 16), 0);
        let new = render::geometry_with_inset(&vol, &app, 100, 40, 4, (8, 16), 8);
        assert_eq!(old.rows, 35);
        assert_eq!(new.rows, 36);
        assert_eq!(new.height, 36 * 16 - 8);
        assert_eq!(new.width, new.height); // Square volume stays square.
        assert_eq!(1 + new.rows, 37); // Footer starts at row 37, not overlapped.
    }

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
            y_offset: 0,
            footer: 15,
            head: 2,
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
        new.head += 1;
        assert!(layout_changed(old, new));
        new = old;
        new.list_start += 1;
        assert!(!layout_changed(old, new)); // scrolling the text-only list
        assert!(!layout_changed(old, old)); // regular slice redraw
    }
}

#[cfg(test)]
mod picker_style_tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn picker_clipping_keeps_path_suffix_and_terminal_cell_width() {
        assert_eq!(
            picker_clip("/long/path/brain.nii.gz", 13, true),
            "…brain.nii.gz"
        );
        assert_eq!(picker_clip("abcdef", 4, false), "abc…");
        assert_eq!(picker_clip("a\x1bb", 10, false), "a�b");
        assert_eq!(picker_clip("abcdef", 0, false), "");
        assert!(picker_clip("/脑/画像.nii.gz", 12, true).width() <= 12);
        let text = picker_prefixed("❯  10.0  ", "/long/path/brain.nii.gz", 23);
        assert!(text.starts_with("❯  10.0  "));
        assert!(text.ends_with("brain.nii.gz"));
        assert!(text.width() <= 23);
    }

    #[test]
    fn filter_cursor_and_selected_basename_fit_cell_width() {
        assert_eq!(filter_preview("abc", Some(1), 12), "a▏bc");
        assert_eq!(filter_preview("abc", None, 12), "abc▏");
        let clipped = filter_preview("/long/目录/brain.nii.gz", Some(6), 9);
        assert!(clipped.contains('▏'));
        assert!(clipped.width() <= 9);
        let (parent, name) = selected_path_parts("/long/research/brain.nii.gz", 16);
        assert_eq!(name, "brain.nii.gz");
        assert!(parent.starts_with('…'));
        assert!(parent.width() + name.width() <= 16);
        assert_eq!(
            selected_path_parts("dir/sub/", 30),
            ("dir/".to_owned(), "sub/".to_owned())
        );
        assert_eq!(
            selected_path_parts("/", 30),
            ("".to_owned(), "/".to_owned())
        );
    }

    #[test]
    fn layer_list_header_space_and_highlight() {
        assert_eq!(layer_list_rows(0), 0);
        assert_eq!(layer_list_rows(6), 7);
        let mut bytes = Vec::new();
        styled_layer_line(
            &mut bytes,
            3,
            "❯ 1: ",
            "brain.nii.gz",
            " [MAX]",
            80,
            PICKER_FG,
            true,
            false,
            true,
        )
        .unwrap();
        let ansi = String::from_utf8(bytes).unwrap();
        assert!(ansi.contains("48;2;49;50;68"));
        assert!(ansi.contains("brain.nii.gz"));
        assert!(ansi.contains("❯ 1:"));
        let wrapped = wrap_panel_line("anatomy_long_file.nii.gz", 10);
        assert!(wrapped.iter().all(|s| s.width() <= 10));
        assert_eq!(wrapped.concat(), "anatomy_long_file.nii.gz");
    }

    #[test]
    fn picker_panel_fits_supported_terminal_sizes() {
        for cols in [42, 80, 120, 240] {
            for rows in [22, 30, 44, 80] {
                let p = PickerPanel::new(cols, rows);
                assert!(p.left + p.width < cols);
                assert!(p.bottom() < rows - 1);
                let free = p.height as usize - 14;
                for matches in [0usize, 1, 3, 1000] {
                    let list = (free - free / 3).min(matches.max(1));
                    let preview = free - list;
                    assert!(list > 0 && preview > 0);
                    assert_eq!(p.top + 7 + list as u16 + 2 + preview as u16, p.bottom() - 4);
                }
                assert_eq!(p.top + 7 + (p.height - 12), p.bottom() - 4);
            }
        }
    }

    #[test]
    fn picker_highlight_uses_truecolor_and_keeps_marker() {
        let mut out = Vec::new();
        let p = PickerPanel::new(80, 24);
        p.row(&mut out, 8, "❯ brain.nii.gz", PICKER_ACCENT, true, false)
            .unwrap();
        let ansi = String::from_utf8(out).unwrap();
        assert!(ansi.contains("❯ brain.nii.gz"));
        assert!(ansi.contains("48;2;49;50;68"));
    }
}

#[cfg(test)]
mod runtime_reference_tests {
    use super::*;
    use std::path::Path;
    fn volume() -> Volume {
        Volume {
            data: ndarray::Array3::zeros((2, 2, 2)),
            spacing: [1.; 3],
            spatial_units: 2,
            sform: None,
            datatype: 16,
        }
    }
    #[test]
    fn revealing_existing_template_moves_it_to_bottom_without_reload() {
        let vol = volume();
        let mut app = App::new(volume::Axis::X, 0, 1, 90, (0., 1.), "base".into());
        let mut layers = Layers::default();
        layers.overlays.push(layers::IntensityOverlay {
            path: "template.nii.gz".into(),
            name: "template".into(),
            vol: volume(),
            window: (0., 10.),
            default_window: (0., 10.),
            data_range: (0., 10.),
            palette: layers::Palette::Gray,
            visible: false,
            opacity: 0.6,
        });
        app.sync_layer_order(1);
        assert_eq!(app.layer_order, vec![0, 1]);
        reveal_template(
            Path::new("template.nii.gz"),
            &vol,
            Path::new("base.nii.gz"),
            &mut app,
            &mut layers,
        )
        .unwrap();
        assert_eq!(app.layer_order, vec![1, 0]);
        assert_eq!(app.selected_layer_id(), 0);
        assert!(layers.overlays[0].visible);
        assert_eq!(layers.overlays[0].opacity, 0.6);
        reveal_template(
            Path::new("template.nii.gz"),
            &vol,
            Path::new("base.nii.gz"),
            &mut app,
            &mut layers,
        )
        .unwrap();
        assert_eq!(layers.overlays.len(), 1);
    }
    #[test]
    fn failed_template_load_keeps_the_stack() {
        let vol = volume();
        let mut app = App::new(volume::Axis::X, 0, 1, 90, (0., 1.), "base".into());
        let mut layers = Layers::default();
        let missing = std::env::temp_dir().join(format!(
            "niv-missing-template-{}.nii.gz",
            std::process::id()
        ));
        assert!(reveal_template(
            &missing,
            &vol,
            Path::new("base.nii.gz"),
            &mut app,
            &mut layers
        )
        .is_err());
        assert_eq!(app.layer_order, vec![0]);
        assert!(layers.overlays.is_empty());
    }
}

#[cfg(test)]
mod range_hint_tests {
    use super::*;
    #[test]
    fn hint_reports_data_range_not_display_window() {
        assert_eq!(
            range_edit_hint(EditField::Minimum, (0.9999001, 1.), (0., 1.)),
            "Data: 0..1 | current: 0.9999001"
        );
        assert_eq!(
            range_edit_hint(EditField::Both, (2., 8.), (-3., 12.)),
            "Data: -3..12 | current: 2,8"
        );
    }
}
