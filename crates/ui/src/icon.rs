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

/// The icon as straight (non-premultiplied) RGBA, `size` x `size`, which is what
/// `tray-icon` and `CreateIconIndirect` with a 32-bit DIB expect.
pub fn render_rgba(size: u32, rings: &[Ring], badge_svg: &str, style: IconStyle) -> Vec<u8> {
    render_pixmap(size, rings, badge_svg, style)
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}
