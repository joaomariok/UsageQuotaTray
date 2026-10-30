//! The runtime-rendered tray icon: rings with color bands around the Badge.

use uqt_core::settings::Gauge;
use uqt_ui::icon::{
    IconLook, IconStyle, app_icon_png, number_text, render_gauge, render_icon, render_rgba,
};
use uqt_ui::present::{Ring, Tone};

#[test]
fn the_notification_icon_is_a_png() {
    let png = app_icon_png(64).expect("png");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn the_dot_marks_the_top_right_corner_in_red() {
    let size = 32;
    let rings = [ring(0.3, Tone::Green)];
    let with_dot = render_icon(size, &rings, BADGE, IconStyle::DarkTaskbar, true);
    let corner = pixel(&with_dot, size, size - 4, 3);
    assert_eq!(corner[3], 255);
    assert!(
        corner[0] > 180 && corner[1] < 100 && corner[2] < 100,
        "{corner:?}"
    );

    let without = render_icon(size, &rings, BADGE, IconStyle::DarkTaskbar, false);
    assert_eq!(pixel(&without, size, size - 4, 3)[3], 0);
    assert_eq!(
        without,
        render_rgba(size, &rings, BADGE, IconStyle::DarkTaskbar)
    );
}

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

fn look(gauge: Gauge, show_badge: bool) -> IconLook {
    IconLook { gauge, show_badge }
}

fn gauge(size: u32, rings: &[Ring], gauge: Gauge, show_badge: bool) -> Vec<u8> {
    render_gauge(
        size,
        rings,
        BADGE,
        IconStyle::DarkTaskbar,
        false,
        look(gauge, show_badge),
    )
}

fn is_green(p: [u8; 4]) -> bool {
    p[3] == 255 && p[1] > 140 && p[0] < 90 && p[2] < 100
}

fn is_amber(p: [u8; 4]) -> bool {
    p[3] == 255 && p[0] > 180 && p[1] > 120 && p[2] < 80
}

fn is_red(p: [u8; 4]) -> bool {
    p[3] == 255 && p[0] > 180 && p[1] < 100 && p[2] < 100
}

fn is_badge(p: [u8; 4]) -> bool {
    p == [0x7A, 0x5A, 0xF8, 255]
}

/// How many pixels in rows `top..bottom` match.
fn count(rgba: &[u8], size: u32, top: u32, bottom: u32, f: impl Fn([u8; 4]) -> bool) -> usize {
    (top..bottom)
        .flat_map(|y| (0..size).map(move |x| (x, y)))
        .filter(|&(x, y)| f(pixel(rgba, size, x, y)))
        .count()
}

#[test]
fn every_gauge_renders_at_every_tray_size() {
    let rings = [ring(0.5, Tone::Green), ring(0.2, Tone::Amber)];
    for g in [
        Gauge::Rings,
        Gauge::Bars,
        Gauge::Number,
        Gauge::StackedNumbers,
    ] {
        for size in [16, 20, 24, 32] {
            for badge in [true, false] {
                assert_eq!(
                    gauge(size, &rings, g, badge).len(),
                    (size * size * 4) as usize
                );
            }
            assert_eq!(gauge(size, &[], g, true).len(), (size * size * 4) as usize);
        }
    }
}

#[test]
fn the_default_look_is_todays_rings_with_the_badge() {
    let rings = [ring(0.4, Tone::Green), ring(0.7, Tone::Amber)];
    let looked = render_gauge(
        32,
        &rings,
        BADGE,
        IconStyle::DarkTaskbar,
        true,
        IconLook::default(),
    );
    assert_eq!(
        looked,
        render_icon(32, &rings, BADGE, IconStyle::DarkTaskbar, true)
    );
}

#[test]
fn rings_without_the_badge_leave_the_centre_empty() {
    let rgba = gauge(32, &[ring(0.5, Tone::Green)], Gauge::Rings, false);
    assert_eq!(pixel(&rgba, 32, 16, 16)[3], 0);
    assert!(is_green(pixel(&rgba, 32, 18, 1)));
}

#[test]
fn bars_fill_left_to_right_with_the_first_meter_on_top() {
    let size = 32;
    let rgba = gauge(
        size,
        &[ring(0.5, Tone::Green), ring(0.75, Tone::Red)],
        Gauge::Bars,
        true,
    );
    // The two bars share the bottom half: green above red.
    let green_rows: Vec<u32> = (0..size)
        .filter(|&y| count(&rgba, size, y, y + 1, is_green) > 0)
        .collect();
    let red_rows: Vec<u32> = (0..size)
        .filter(|&y| count(&rgba, size, y, y + 1, is_red) > 0)
        .collect();
    assert!(!green_rows.is_empty() && !red_rows.is_empty());
    assert!(green_rows.iter().max() < red_rows.iter().min());
    assert!(*green_rows.first().unwrap() >= size / 2);

    // Filled from the left up to the fraction, then the track.
    let y = green_rows[green_rows.len() / 2];
    assert!(is_green(pixel(&rgba, size, 2, y)));
    assert!(!is_green(pixel(&rgba, size, size - 3, y)));
    assert!(pixel(&rgba, size, size - 3, y)[3] > 0, "the track shows");
    let y = red_rows[red_rows.len() / 2];
    assert!(is_red(pixel(&rgba, size, size * 3 / 4 - 3, y)));
    assert!(!is_red(pixel(&rgba, size, size - 3, y)));

    // The Badge sits above the bars.
    assert!(count(&rgba, size, 0, size / 2, is_badge) > 0);
}

#[test]
fn bars_without_the_badge_take_the_whole_icon() {
    let size = 32;
    let with = gauge(
        size,
        &[ring(0.5, Tone::Green), ring(0.5, Tone::Green)],
        Gauge::Bars,
        true,
    );
    let without = gauge(
        size,
        &[ring(0.5, Tone::Green), ring(0.5, Tone::Green)],
        Gauge::Bars,
        false,
    );
    assert_eq!(count(&without, size, 0, size, is_badge), 0);
    assert!(count(&without, size, 0, size, is_green) > count(&with, size, 0, size, is_green));
    assert!(count(&without, size, 0, size / 2, is_green) > 0);
}

#[test]
fn numbers_read_as_the_percentage_used() {
    assert_eq!(number_text(Some(&ring(0.42, Tone::Green))), "42");
    assert_eq!(number_text(Some(&ring(0.0, Tone::Green))), "0");
    assert_eq!(number_text(Some(&ring(0.996, Tone::Red))), "99");
    assert_eq!(number_text(Some(&ring(1.0, Tone::Red))), "!!");
    assert_eq!(number_text(Some(&ring(1.4, Tone::Red))), "!!");
    // Stale or past its reset: no number to trust.
    assert_eq!(number_text(Some(&ring(0.42, Tone::Grey))), "--");
    assert_eq!(number_text(None), "--");
}

#[test]
fn the_number_is_drawn_in_its_band_colour_without_the_badge() {
    let size = 32;
    let rgba = gauge(
        size,
        &[ring(0.8, Tone::Amber), ring(0.1, Tone::Green)],
        Gauge::Number,
        true,
    );
    assert!(count(&rgba, size, 0, size, is_amber) > 20);
    assert_eq!(
        count(&rgba, size, 0, size, is_green),
        0,
        "only the first Meter"
    );
    assert_eq!(count(&rgba, size, 0, size, is_badge), 0);
    assert_ne!(
        rgba,
        gauge(size, &[ring(0.9, Tone::Amber)], Gauge::Number, true),
        "a different number draws differently"
    );
}

#[test]
fn stacked_numbers_show_the_first_meter_above_the_second() {
    let size = 32;
    let rgba = gauge(
        size,
        &[ring(0.8, Tone::Amber), ring(1.2, Tone::Red)],
        Gauge::StackedNumbers,
        true,
    );
    assert!(count(&rgba, size, 0, size / 2, is_amber) > 10);
    assert_eq!(count(&rgba, size, size / 2, size, is_amber), 0);
    assert!(count(&rgba, size, size / 2, size, is_red) > 5);
    assert_eq!(count(&rgba, size, 0, size / 2, is_red), 0);
    assert_eq!(count(&rgba, size, 0, size, is_badge), 0);
}

#[test]
fn digits_are_drawn_darker_on_a_light_taskbar() {
    let size = 32;
    let on = |style| {
        render_gauge(
            size,
            &[ring(0.42, Tone::Amber)],
            BADGE,
            style,
            false,
            look(Gauge::Number, false),
        )
    };
    let brightest = |rgba: &[u8]| {
        rgba.chunks(4)
            .filter(|p| p[3] == 255)
            .map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32)
            .max()
            .unwrap()
    };
    let (dark, light) = (on(IconStyle::DarkTaskbar), on(IconStyle::LightTaskbar));
    assert!(brightest(&light) + 100 < brightest(&dark));
    // Still amber, not a shade of grey.
    let p = light.chunks(4).find(|p| p[3] == 255).unwrap();
    assert!(p[0] > p[2] + 80 && p[1] > p[2] + 40, "{p:?}");
}

#[test]
fn the_holes_in_digits_stay_open_at_16_px() {
    let size = 16;
    for style in [IconStyle::DarkTaskbar, IconStyle::LightTaskbar] {
        let rgba = render_gauge(
            size,
            &[ring(0.88, Tone::Amber)],
            BADGE,
            style,
            false,
            look(Gauge::Number, false),
        );
        let opaque: Vec<(u32, u32)> = (0..size)
            .flat_map(|y| (0..size).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&rgba, size, x, y)[3] > 0)
            .collect();
        let (l, r) = (
            opaque.iter().map(|p| p.0).min().unwrap(),
            opaque.iter().map(|p| p.0).max().unwrap() + 1,
        );
        let (t, b) = (
            opaque.iter().map(|p| p.1).min().unwrap(),
            opaque.iter().map(|p| p.1).max().unwrap() + 1,
        );
        // "88" spans 7 grid columns and 5 rows; the first hole is cell (1, 1).
        let x = l + (r - l) * 3 / 14;
        let y = t + (b - t) * 3 / 10;
        assert_eq!(
            pixel(&rgba, size, x, y)[3],
            0,
            "{style:?}: hole at ({x}, {y})"
        );
    }
}

#[test]
fn on_numbers_the_dot_is_small_and_clears_nothing() {
    let size = 16;
    let rings = [ring(0.88, Tone::Green), ring(0.77, Tone::Green)];
    for g in [Gauge::Number, Gauge::StackedNumbers] {
        let draw = |dot| {
            render_gauge(
                size,
                &rings,
                BADGE,
                IconStyle::DarkTaskbar,
                dot,
                look(g, false),
            )
        };
        let (plain, dotted) = (draw(false), draw(true));
        let changed: Vec<usize> = (0..(size * size) as usize)
            .filter(|i| plain[i * 4..i * 4 + 4] != dotted[i * 4..i * 4 + 4])
            .collect();
        assert!(!changed.is_empty(), "{g:?}: the dot shows");
        assert!(
            changed.len() <= 16,
            "{g:?}: {} pixels changed",
            changed.len()
        );
        for &i in &changed {
            let (x, y) = ((i as u32) % size, (i as u32) / size);
            assert!(
                x >= size - 5 && y < 5,
                "{g:?}: ({x}, {y}) outside the corner"
            );
            assert!(dotted[i * 4 + 3] > 0, "{g:?}: ({x}, {y}) was cleared");
        }
    }
}
