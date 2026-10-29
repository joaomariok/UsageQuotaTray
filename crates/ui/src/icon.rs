//! Renders the tray icon: up to two Meters as concentric rings around the Badge.
//! Ported from the UI prototype's icon crate.

use std::f32::consts::{FRAC_PI_2, TAU};

use resvg::tiny_skia::{Color, LineCap, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};
use resvg::usvg;

use crate::present::{Ring, Tone};

/// Which taskbar the icon sits on, so the track and grey stay visible on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconStyle {
    DarkTaskbar,
    LightTaskbar,
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

fn render_pixmap(size: u32, rings: &[Ring], badge_svg: &str, style: IconStyle) -> Pixmap {
    let mut pm = Pixmap::new(size.max(1), size.max(1)).expect("non-zero icon size");
    let s = pm.width() as f32;
    let centre = s / 2.0;
    let stroke_w = (s / 8.0).max(2.0);
    let gap = 1.0;
    let track = match style {
        IconStyle::DarkTaskbar => Color::from_rgba8(255, 255, 255, 70),
        IconStyle::LightTaskbar => Color::from_rgba8(0, 0, 0, 60),
    };
    let stroke = Stroke {
        width: stroke_w,
        line_cap: LineCap::Butt,
        ..Stroke::default()
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };

    let rings = &rings[..rings.len().min(2)];
    for (i, ring) in rings.iter().enumerate() {
        let r = centre - stroke_w / 2.0 - i as f32 * (stroke_w + gap);
        paint.set_color(track);
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
    let inner = s - 2.0 * rings.len() as f32 * (stroke_w + gap);
    let box_size = (inner * 0.85).max(4.0);
    match usvg::Tree::from_str(badge_svg, &usvg::Options::default()) {
        Ok(tree) => {
            let sz = tree.size();
            let scale = box_size / sz.width().max(sz.height());
            let tx = (s - sz.width() * scale) / 2.0;
            let ty = (s - sz.height() * scale) / 2.0;
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
            paint.set_color(tone_color(Tone::Grey, style));
            if let Some(dot) = PathBuilder::from_circle(centre, centre, box_size / 4.0) {
                pm.fill_path(
                    &dot,
                    &paint,
                    resvg::tiny_skia::FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
    pm
}

/// Marks the top-right corner with a red dot inside a transparent halo, so it reads
/// on top of the rings on any taskbar.
fn draw_dot(pm: &mut Pixmap) {
    let s = pm.width() as f32;
    let r = (s * 0.2).max(2.5);
    let (cx, cy) = (s - r - 0.5, r + 0.5);
    let fill = |pm: &mut Pixmap, radius: f32, paint: &Paint<'_>| {
        if let Some(c) = PathBuilder::from_circle(cx, cy, radius) {
            pm.fill_path(
                &c,
                paint,
                resvg::tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    };
    let clear = Paint {
        anti_alias: true,
        blend_mode: resvg::tiny_skia::BlendMode::Clear,
        ..Paint::default()
    };
    fill(pm, r + 1.0, &clear);
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
    render_pixmap(size, &rings, "", IconStyle::DarkTaskbar)
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
    let mut pm = render_pixmap(size, rings, badge_svg, style);
    if dot {
        draw_dot(&mut pm);
    }
    pm.pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}
