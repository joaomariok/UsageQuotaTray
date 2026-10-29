//! The runtime-rendered tray icon: rings with color bands around the Badge.

use uqt_ui::icon::{IconStyle, render_rgba};
use uqt_ui::present::{Ring, Tone};

const BADGE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#7A5AF8" d="M8 8h8v8H8z"/></svg>"##;

fn pixel(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn ring(fraction: f32, tone: Tone) -> Ring {
    Ring { fraction, tone }
}

#[test]
fn renders_straight_rgba_at_the_requested_size() {
    for size in [16, 20, 24, 32] {
        let rgba = render_rgba(
            size,
            &[ring(0.5, Tone::Green)],
            BADGE,
            IconStyle::DarkTaskbar,
        );
        assert_eq!(rgba.len(), (size * size * 4) as usize);
    }
}

#[test]
fn the_outer_ring_fills_clockwise_from_the_top_in_its_band_color() {
    let size = 32;
    let rgba = render_rgba(
        size,
        &[ring(0.42, Tone::Green)],
        BADGE,
        IconStyle::DarkTaskbar,
    );
    // Just right of top centre, on the outer ring: filled, green, opaque.
    let top = pixel(&rgba, size, size / 2 + 1, 2);
    assert_eq!(top[3], 255);
    assert!(top[1] > top[0] && top[1] > top[2], "green: {top:?}");
    // Left middle is 75% of the way round: only the faint track.
    let left = pixel(&rgba, size, 2, size / 2);
    assert!(left[3] < 128, "track: {left:?}");
}

#[test]
fn red_and_amber_bands_are_distinguishable() {
    let size = 32;
    let red = pixel(
        &render_rgba(
            size,
            &[ring(0.95, Tone::Red)],
            BADGE,
            IconStyle::LightTaskbar,
        ),
        size,
        size / 2 + 1,
        2,
    );
    let amber = pixel(
        &render_rgba(
            size,
            &[ring(0.8, Tone::Amber)],
            BADGE,
            IconStyle::LightTaskbar,
        ),
        size,
        size / 2 + 1,
        2,
    );
    assert!(red[0] > 150 && red[1] < 100, "red: {red:?}");
    assert!(
        amber[0] > 150 && amber[1] > 120 && amber[2] < 80,
        "amber: {amber:?}"
    );
}

#[test]
fn the_track_contrasts_with_light_and_dark_taskbars() {
    let size = 32;
    let empty = [ring(0.0, Tone::Green)];
    let on_dark = pixel(
        &render_rgba(size, &empty, BADGE, IconStyle::DarkTaskbar),
        size,
        2,
        size / 2,
    );
    let on_light = pixel(
        &render_rgba(size, &empty, BADGE, IconStyle::LightTaskbar),
        size,
        2,
        size / 2,
    );
    assert!(
        on_dark[0] > 200,
        "light track on a dark taskbar: {on_dark:?}"
    );
    assert!(
        on_light[0] < 60,
        "dark track on a light taskbar: {on_light:?}"
    );
    assert!(on_dark[3] > 0 && on_light[3] > 0);
}

#[test]
fn the_badge_is_drawn_in_the_centre() {
    let size = 32;
    let rgba = render_rgba(
        size,
        &[ring(0.5, Tone::Green), ring(0.5, Tone::Green)],
        BADGE,
        IconStyle::DarkTaskbar,
    );
    assert_eq!(
        pixel(&rgba, size, size / 2, size / 2),
        [0x7A, 0x5A, 0xF8, 255]
    );
}

#[test]
fn an_unrenderable_badge_still_produces_an_icon() {
    let rgba = render_rgba(
        16,
        &[ring(0.5, Tone::Green)],
        "<nope",
        IconStyle::DarkTaskbar,
    );
    assert_eq!(rgba.len(), 16 * 16 * 4);
}
