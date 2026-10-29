//! Keyboard movement in the settings navigation list. The Slint `NavList` reports a key
//! as a small code and asks [`step`] where the selection goes, so the rule lives (and is
//! tested) here rather than in the markup.

/// A key the navigation list reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Home,
    End,
}

impl NavKey {
    /// The code `NavList` passes: 0 Up, 1 Down, 2 Home, 3 End.
    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Up),
            1 => Some(Self::Down),
            2 => Some(Self::Home),
            3 => Some(Self::End),
            _ => None,
        }
    }
}

/// The entry selected after `key`, in a list of `len` entries. Arrows stop at the ends;
/// a `current` outside the list is pulled back in first. An empty list stays at 0.
pub fn step(current: usize, len: usize, key: NavKey) -> usize {
    let Some(last) = len.checked_sub(1) else {
        return 0;
    };
    let current = current.min(last);
    match key {
        NavKey::Up => current.saturating_sub(1),
        NavKey::Down => (current + 1).min(last),
        NavKey::Home => 0,
        NavKey::End => last,
    }
}
