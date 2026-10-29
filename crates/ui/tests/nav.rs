//! Keyboard behaviour of the settings navigation list (the seam is `uqt_ui::nav::step`).

use uqt_ui::nav::{NavKey, step};

#[test]
fn down_and_up_move_one_entry() {
    assert_eq!(step(0, 3, NavKey::Down), 1);
    assert_eq!(step(2, 3, NavKey::Up), 1);
}

#[test]
fn arrows_stop_at_the_ends_instead_of_wrapping() {
    assert_eq!(step(2, 3, NavKey::Down), 2);
    assert_eq!(step(0, 3, NavKey::Up), 0);
}

#[test]
fn home_and_end_jump_to_the_ends() {
    assert_eq!(step(1, 3, NavKey::Home), 0);
    assert_eq!(step(1, 3, NavKey::End), 2);
}

#[test]
fn an_empty_list_stays_at_zero() {
    assert_eq!(step(0, 0, NavKey::Down), 0);
    assert_eq!(step(0, 0, NavKey::End), 0);
}

#[test]
fn an_out_of_range_current_is_pulled_back_in() {
    assert_eq!(step(7, 3, NavKey::Up), 1);
    assert_eq!(step(7, 3, NavKey::Down), 2);
}

#[test]
fn slint_key_codes_map_to_nav_keys() {
    assert_eq!(NavKey::from_code(0), Some(NavKey::Up));
    assert_eq!(NavKey::from_code(1), Some(NavKey::Down));
    assert_eq!(NavKey::from_code(2), Some(NavKey::Home));
    assert_eq!(NavKey::from_code(3), Some(NavKey::End));
    assert_eq!(NavKey::from_code(4), None);
    assert_eq!(NavKey::from_code(-1), None);
}
