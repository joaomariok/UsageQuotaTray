//! Renders the tray icon: up to two Featured Meters in the user's Gauge (rings, bars
//! or numbers), with the Badge when the Gauge has room for it. The rings were ported
//! from the UI prototype's icon crate.

use std::f32::consts::{FRAC_PI_2, TAU};

use resvg::tiny_skia::{
    Color, FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};
use resvg::usvg;
use uqt_core::settings::{Gauge, Settings};

use crate::present::{Ring, Tone};

/// Which taskbar the icon sits on, so the track and grey stay visible on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconStyle {
    DarkTaskbar,
    LightTaskbar,
}

/// How the user wants the icon drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconLook {
    pub gauge: Gauge,
    /// Ignored by the Gauges that have no room for the Badge.
    pub show_badge: bool,
}

impl From<&Settings> for IconLook {
    fn from(s: &Settings) -> Self {
        Self {
            gauge: s.gauge,
            show_badge: s.show_badge,
        }
    }
}

impl Default for IconLook {
    fn default() -> Self {
        Self {
            gauge: Gauge::default(),
            show_badge: true,
        }
    }
}

fn tone_color(tone: Tone, style: IconStyle) -> Color {
    match tone {
        Tone::Green => Color::from_rgba8(0x2E, 0xA0, 0x43, 0xFF),
        Tone::Amber => Color::from_rgba8(0xE3, 0xA0, 0x08, 0xFF),
        Tone::Red => Color::from_rgba8(0xD7, 0x3A, 0x49, 0xFF),
        Tone::Grey => match style {
            IconStyle::DarkTaskbar => Color::from_rgba8(0xA0, 0xA0, 0xA0, 0xFF),
            IconStyle::LightTaskbar => Color::from_rgba8(0x6E, 0x6E, 0x6E, 0xFF),
        },
    }
}

/// Digits are thin, so on a light taskbar they take darker shades of the bands to
/// keep their contrast.
fn digit_color(tone: Tone, style: IconStyle) -> Color {
    match (tone, style) {
        (Tone::Green, IconStyle::LightTaskbar) => Color::from_rgba8(0x1A, 0x6E, 0x2C, 0xFF),
        (Tone::Amber, IconStyle::LightTaskbar) => Color::from_rgba8(0x9A, 0x66, 0x00, 0xFF),
        (Tone::Red, IconStyle::LightTaskbar) => Color::from_rgba8(0xA8, 0x1E, 0x2E, 0xFF),
        _ => tone_color(tone, style),
    }
}

fn track_color(style: IconStyle) -> Color {
    match style {
        IconStyle::DarkTaskbar => Color::from_rgba8(255, 255, 255, 70),
        IconStyle::LightTaskbar => Color::from_rgba8(0, 0, 0, 60),
    }
}

/// tiny-skia has no arc primitive, so approximate one with a polyline.
fn arc(cx: f32, cy: f32, r: f32, from: f32, sweep: f32) -> Option<Path> {
    let steps = ((sweep.abs() / TAU) * 96.0).ceil().max(2.0) as usize;
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = from + sweep * (i as f32 / steps as f32);
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y)
        } else {
            pb.line_to(x, y)
        }
    }
    pb.finish()
}

fn render_pixmap(
    size: u32,
    rings: &[Ring],
    badge_svg: &str,
    style: IconStyle,
    look: IconLook,
) -> Pixmap {
    let mut pm = Pixmap::new(size.max(1), size.max(1)).expect("non-zero icon size");
    let rings = &rings[..rings.len().min(2)];
    let badge = look.show_badge.then_some(badge_svg);
    match look.gauge {
        Gauge::Rings => draw_rings(&mut pm, rings, badge, style),
        Gauge::Bars => draw_bars(&mut pm, rings, badge, style),
        Gauge::Number => draw_numbers(&mut pm, &rings[..rings.len().min(1)], 1, style),
        Gauge::StackedNumbers => draw_numbers(&mut pm, rings, 2, style),
    }
    pm
}

fn draw_rings(pm: &mut Pixmap, rings: &[Ring], badge_svg: Option<&str>, style: IconStyle) {
    let s = pm.width() as f32;
    let centre = s / 2.0;
    let stroke_w = (s / 8.0).max(2.0);
    let gap = 1.0;
    let stroke = Stroke {
        width: stroke_w,
        line_cap: LineCap::Butt,
        ..Stroke::default()
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };

    for (i, ring) in rings.iter().enumerate() {
        let r = centre - stroke_w / 2.0 - i as f32 * (stroke_w + gap);
        paint.set_color(track_color(style));
        if let Some(p) = arc(centre, centre, r, 0.0, TAU) {
            pm.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
        }
        let used = ring.fraction.clamp(0.0, 1.0);
        if used > 0.0 {
            paint.set_color(tone_color(ring.tone, style));
            if let Some(p) = arc(centre, centre, r, -FRAC_PI_2, TAU * used) {
                pm.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
            }
        }
    }

    // The Badge fills most of the space left inside the inner ring.
    if let Some(svg) = badge_svg {
        let inner = s - 2.0 * rings.len() as f32 * (stroke_w + gap);
        draw_badge(pm, svg, (centre, centre), (inner * 0.85).max(4.0), style);
    }
}

/// Up to two horizontal bars along the bottom, first Meter on top, each filling from
/// the left. Without the Badge they grow to take the whole icon.
fn draw_bars(pm: &mut Pixmap, rings: &[Ring], badge_svg: Option<&str>, style: IconStyle) {
    let size = pm.width();
    let s = size as f32;
    let gap = (s / 16.0).round().max(1.0) as u32;
    let n = rings.len() as u32;
    let bar_h = match badge_svg {
        Some(_) => (size / 2).saturating_sub(gap) / 2,
        None => (s * 0.4).round() as u32,
    }
    .max(1);
    let total = (n * bar_h + n.saturating_sub(1) * gap).min(size);
    let top = match badge_svg {
        Some(_) => size - total,
        None => (size - total) / 2,
    };
    for (i, ring) in rings.iter().enumerate() {
        let y = (top + i as u32 * (bar_h + gap)) as f32;
        let bottom = y + bar_h as f32;
        fill_rect(pm, Rect::from_ltrb(0.0, y, s, bottom), track_color(style));
        let used = (s * ring.fraction.clamp(0.0, 1.0)).round();
        fill_rect(
            pm,
            Rect::from_ltrb(0.0, y, used, bottom),
            tone_color(ring.tone, style),
        );
    }
    // The Badge fills the space above the bars.
    if let Some(svg) = badge_svg {
        let room = if n == 0 {
            s
        } else {
            top.saturating_sub(gap) as f32
        };
        draw_badge(pm, svg, (s / 2.0, room / 2.0), (room * 0.9).max(4.0), style);
    }
}

/// The text a number Gauge shows for one Featured Meter: the percentage used, `!!` at
/// or past the Limit, and `--` when there is no value to trust (no data, stale, or
/// past its reset).
pub fn number_text(ring: Option<&Ring>) -> String {
    match ring {
        Some(r) if r.tone != Tone::Grey => {
            if r.fraction >= 1.0 {
                "!!".into()
            } else {
                // Rounded like the popup, but never up to 100 before the Limit.
                ((r.fraction.max(0.0) * 100.0).round() as u32)
                    .min(99)
                    .to_string()
            }
        }
        _ => "--".into(),
    }
}

/// `rows` numbers one above the other, filling the icon, each in its band colour.
fn draw_numbers(pm: &mut Pixmap, rings: &[Ring], rows: u32, style: IconStyle) {
    let s = pm.width() as f32;
    let margin = (s / 24.0).round().max(1.0);
    let gap = (s / 16.0).round().max(1.0);
    let row_h = (s - 2.0 * margin - (rows - 1) as f32 * gap) / rows as f32;
    for row in 0..rows as usize {
        let ring = rings.get(row);
        let tone = ring.map_or(Tone::Grey, |r| r.tone);
        let y = margin + row as f32 * (row_h + gap);
        draw_text(
            pm,
            &number_text(ring),
            Rect::from_xywh(margin, y, s - 2.0 * margin, row_h),
            digit_color(tone, style),
        );
    }
}

/// A character on a 3x5 grid, one string per row, `#` lit. Anything that is not a
/// digit or `!` draws as a dash.
fn glyph(c: char) -> [&'static str; 5] {
    match c {
        '0' => ["###", "#.#", "#.#", "#.#", "###"],
        '1' => [".#.", "##.", ".#.", ".#.", "###"],
        '2' => ["###", "..#", "###", "#..", "###"],
        '3' => ["###", "..#", "###", "..#", "###"],
        '4' => ["#.#", "#.#", "###", "..#", "..#"],
        '5' => ["###", "#..", "###", "..#", "###"],
        '6' => ["###", "#..", "###", "#.#", "###"],
        '7' => ["###", "..#", "..#", "..#", "..#"],
        '8' => ["###", "#.#", "###", "#.#", "###"],
        '9' => ["###", "#.#", "###", "..#", "###"],
        '!' => [".#.", ".#.", ".#.", "...", ".#."],
        _ => ["...", "...", "###", "...", "..."],
    }
}

/// Draws `text` centred in `area`. Each grid cell is snapped to whole pixels, so the
/// digits stay sharp at 16 px.
fn draw_text(pm: &mut Pixmap, text: &str, area: Option<Rect>, color: Color) {
    let Some(area) = area else { return };
    let (ax, ay, aw, ah) = (area.x(), area.y(), area.width(), area.height());
    let chars: Vec<char> = text.chars().collect();
    let cols = (chars.len() * 4).saturating_sub(1).max(1) as f32;
    let ch = ah / 5.0;
    let cw = (aw / cols).min(ch * 1.6);
    let ox = ax + (aw - cw * cols) / 2.0;
    for (i, &c) in chars.iter().enumerate() {
        for (r, line) in glyph(c).into_iter().enumerate() {
            for (k, _) in line.char_indices().filter(|&(_, b)| b == '#') {
                let col = (i * 4 + k) as f32;
                let cell = Rect::from_ltrb(
                    (ox + col * cw).round(),
                    (ay + r as f32 * ch).round(),
                    (ox + (col + 1.0) * cw).round(),
                    (ay + (r + 1) as f32 * ch).round(),
                );
                fill_rect(pm, cell, color);
            }
        }
    }
}

/// Fills `rect` without anti-aliasing; an empty one (`None`) draws nothing.
fn fill_rect(pm: &mut Pixmap, rect: Option<Rect>, color: Color) {
    if let Some(rect) = rect {
        let mut paint = Paint::default();
        paint.set_color(color);
        pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

/// The Badge scaled to fit `box_size` around `centre`, or a grey dot in its place when
/// it does not render.
fn draw_badge(
    pm: &mut Pixmap,
    badge_svg: &str,
    (cx, cy): (f32, f32),
    box_size: f32,
    style: IconStyle,
) {
    match usvg::Tree::from_str(badge_svg, &usvg::Options::default()) {
        Ok(tree) => {
            let sz = tree.size();
            let scale = box_size / sz.width().max(sz.height());
            let tx = cx - sz.width() * scale / 2.0;
            let ty = cy - sz.height() * scale / 2.0;
            resvg::render(
                &tree,
                Transform::from_row(scale, 0.0, 0.0, scale, tx, ty),
                &mut pm.as_mut(),
            );
        }
        Err(e) => {
            // An empty Badge is the placeholder before any Account has data.
            if !badge_svg.is_empty() {
                log::warn!("badge does not render: {e}");
            }
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color(tone_color(Tone::Grey, style));
            if let Some(dot) = PathBuilder::from_circle(cx, cy, box_size / 4.0) {
                pm.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
    }
}

/// Marks the top-right corner with a red dot inside a transparent halo, so it reads
/// on top of the rings on any taskbar. Numbers fill the whole icon, so on them the dot
/// is small and clears nothing: it covers only a digit's corner cell.
fn draw_dot(pm: &mut Pixmap, on_numbers: bool) {
    let s = pm.width() as f32;
    let r = if on_numbers {
        (s * 0.11).max(1.5)
    } else {
        (s * 0.2).max(2.5)
    };
    let (cx, cy) = (s - r - 0.5, r + 0.5);
    let fill = |pm: &mut Pixmap, radius: f32, paint: &Paint<'_>| {
        if let Some(c) = PathBuilder::from_circle(cx, cy, radius) {
            pm.fill_path(&c, paint, FillRule::Winding, Transform::identity(), None);
        }
    };
    let clear = Paint {
        anti_alias: true,
        blend_mode: resvg::tiny_skia::BlendMode::Clear,
        ..Paint::default()
    };
    if !on_numbers {
        fill(pm, r + 1.0, &clear);
    }
    let mut red = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    red.set_color(tone_color(Tone::Red, IconStyle::DarkTaskbar));
    fill(pm, r, &red);
}

/// The icon as straight (non-premultiplied) RGBA, `size` x `size`, which is what
/// `tray-icon` and `CreateIconIndirect` with a 32-bit DIB expect.
pub fn render_rgba(size: u32, rings: &[Ring], badge_svg: &str, style: IconStyle) -> Vec<u8> {
    render_icon(size, rings, badge_svg, style, false)
}

/// The app's own icon, for the notification identity: two sample rings around the
/// placeholder, as a PNG.
pub fn app_icon_png(size: u32) -> Option<Vec<u8>> {
    let rings = [
        Ring {
            fraction: 0.65,
            tone: Tone::Green,
        },
        Ring {
            fraction: 0.35,
            tone: Tone::Amber,
        },
    ];
    render_pixmap(
        size,
        &rings,
        "",
        IconStyle::DarkTaskbar,
        IconLook::default(),
    )
    .encode_png()
    .ok()
}

/// [`render_rgba`], with the dot that flags a non-featured Meter at 90% or more.
pub fn render_icon(
    size: u32,
    rings: &[Ring],
    badge_svg: &str,
    style: IconStyle,
    dot: bool,
) -> Vec<u8> {
    render_gauge(size, rings, badge_svg, style, dot, IconLook::default())
}

/// [`render_icon`] in the user's [`IconLook`].
pub fn render_gauge(
    size: u32,
    rings: &[Ring],
    badge_svg: &str,
    style: IconStyle,
    dot: bool,
    look: IconLook,
) -> Vec<u8> {
    let mut pm = render_pixmap(size, rings, badge_svg, style, look);
    if dot {
        let on_numbers = matches!(look.gauge, Gauge::Number | Gauge::StackedNumbers);
        draw_dot(&mut pm, on_numbers);
    }
    pm.pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}
