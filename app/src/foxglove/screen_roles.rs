//! A keyboard's screens by role (#560): what each screen is for and what the app calls it,
//! never which side it sits on. The firmware's own descriptors (HELLO capability SCREENS) win;
//! the board profile's `[[screens]]` fill in what the firmware doesn't say, and give each screen
//! its name (`name`, else the role's: "Info screen" for the panel the PC draws, "Art screen" for
//! where art plays, "Display" for one the firmware draws itself).
//!
//! Every string that names a screen comes from here, so a board with one screen, or three of
//! different sizes, reads right.

use digitalis::protocol::{ScreenDescriptor, ScreenKind};

use super::boards::Board;

/// What a screen is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The panel the PC draws (clock, media, hints, plugins).
    Info,
    /// Where art plays.
    Art,
    /// A screen the keyboard's firmware draws itself.
    Display,
}

impl Role {
    /// The role of a profile's `kind`; `None` for a kind Foxglove doesn't know.
    pub fn of_kind(kind: &str) -> Option<Role> {
        match kind {
            "panel" => Some(Role::Info),
            "art" => Some(Role::Art),
            "display" => Some(Role::Display),
            _ => None,
        }
    }

    fn of_descriptor(kind: ScreenKind) -> Option<Role> {
        match kind {
            ScreenKind::Panel => Some(Role::Info),
            ScreenKind::Art => Some(Role::Art),
            ScreenKind::Display => Some(Role::Display),
            ScreenKind::Other(_) => None,
        }
    }

    /// The name a screen of this role has when its profile gives none.
    pub fn default_name(self) -> &'static str {
        match self {
            Role::Info => "Info screen",
            Role::Art => "Art screen",
            Role::Display => "Display",
        }
    }

    /// The plugin API's word for it (`screens.list`).
    pub fn api_name(self) -> &'static str {
        match self {
            Role::Info => "info",
            Role::Art => "art",
            Role::Display => "display",
        }
    }
}

/// One screen: its id (the firmware's, or its place in the profile), role, name and size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub id: u8,
    pub role: Role,
    pub name: String,
    /// As the PC addresses it (the stored buffer: an art screen mounted turned is wider than
    /// the art is).
    pub width: u16,
    pub height: u16,
    /// `1bit`; other formats aren't drawn yet.
    pub format: String,
    /// 90-degree steps clockwise from upright to how it is stored.
    pub rotation: u8,
}

impl Screen {
    /// Its size upright, as the user sees it.
    pub fn upright(&self) -> (u16, u16) {
        if self.rotation % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// Its name within a sentence: "the art screen".
    pub fn the(&self) -> String {
        format!("the {}", super::parts::lower_first(&self.name))
    }
}

/// A board's screens from its profile, ids by their place in it.
pub fn of_board(board: &Board) -> Vec<Screen> {
    board
        .screens
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let role = Role::of_kind(&s.kind)?;
            Some(Screen {
                id: u8::try_from(i).ok()?,
                role,
                name: s
                    .name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| role.default_name().to_string()),
                width: s.width,
                height: s.height,
                format: s.format.clone(),
                rotation: u8::try_from((s.rotate / 90) % 4).unwrap_or(0),
            })
        })
        .collect()
}

/// The screens the firmware describes, named from the profile (the n-th screen of a role takes
/// the profile's n-th of that role). A display that holds panels or art areas isn't one of its
/// own: its regions are.
pub fn of_descriptors(described: &[ScreenDescriptor], board: Option<&Board>) -> Vec<Screen> {
    let named = board.map(of_board).unwrap_or_default();
    let mut seen: Vec<Role> = Vec::new();
    described
        .iter()
        .filter(|d| !described.iter().any(|o| o.parent == Some(d.id)))
        .filter_map(|d| {
            let role = Role::of_descriptor(d.kind)?;
            let nth = seen.iter().filter(|r| **r == role).count();
            seen.push(role);
            let name = named
                .iter()
                .filter(|s| s.role == role)
                .nth(nth)
                .map_or_else(|| role.default_name().to_string(), |s| s.name.clone());
            Some(Screen {
                id: d.id,
                role,
                name,
                width: d.width,
                height: d.height,
                format: if d.format == ScreenDescriptor::FORMAT_1BIT {
                    "1bit".to_string()
                } else {
                    d.format_name().to_string()
                },
                rotation: d.rotation % 4,
            })
        })
        .collect()
}

/// The connected keyboard's screens: the firmware's, else its profile's. `None` while nothing
/// says (no profile, and the firmware hasn't described any).
pub fn current() -> Option<Vec<Screen>> {
    let board = super::boards::current();
    let described = super::screens::snapshot().screens;
    if !described.is_empty() {
        return Some(of_descriptors(&described, board.as_ref()));
    }
    board.as_ref().map(of_board)
}

/// The first screen of `role` in `screens`.
pub fn first(screens: &[Screen], role: Role) -> Option<&Screen> {
    screens.iter().find(|s| s.role == role)
}

/// What the app calls the connected keyboard's screen of `role`: "Info screen", "Art screen", or
/// the profile's name for it.
pub fn name(role: Role) -> String {
    name_in(current().as_deref(), role)
}

fn name_in(screens: Option<&[Screen]>, role: Role) -> String {
    screens
        .and_then(|s| first(s, role))
        .map_or_else(|| role.default_name().to_string(), |s| s.name.clone())
}

/// [`name`] within a sentence: "the info screen".
pub fn the(role: Role) -> String {
    format!("the {}", super::parts::lower_first(&name(role)))
}

/// Whether the connected keyboard has a screen of `role`. Unknown (no profile, nothing
/// described) counts as yes for the info and art screens, so a keyboard Foxglove can't
/// describe keeps the cards it had.
pub fn has(role: Role) -> bool {
    has_in(current().as_deref(), role)
}

fn has_in(screens: Option<&[Screen]>, role: Role) -> bool {
    match screens {
        Some(s) => first(s, role).is_some(),
        None => role != Role::Display,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::fixtures::{second_desk, users_desk};

    #[test]
    fn the_corne_has_an_info_and_an_art_screen() {
        let s = of_board(&users_desk::board());
        let roles: Vec<_> = s.iter().map(|s| (s.role, s.name.as_str())).collect();
        assert_eq!(
            roles,
            [(Role::Info, "Info screen"), (Role::Art, "Art screen")]
        );
        assert_eq!((s[0].width, s[0].height), (64, 52));
        // The art is stored turned: 140x68 stored, 68x140 upright.
        assert_eq!(s[1].upright(), (68, 140));
        assert_eq!(s[1].the(), "the art screen");
    }

    #[test]
    fn the_second_desks_three_screens_are_named_by_role() {
        let s = of_board(&second_desk::board());
        let named: Vec<_> = s
            .iter()
            .map(|s| (s.id, s.role, s.name.as_str(), s.width, s.height))
            .collect();
        assert_eq!(
            named,
            [
                (0, Role::Info, "Info screen", 128, 32),
                (1, Role::Art, "Art screen", 160, 80),
                (2, Role::Display, "Display", 240, 135),
            ]
        );
        assert!(s
            .iter()
            .all(|s| !s.name.to_lowercase().contains("left")
                && !s.name.to_lowercase().contains("right")));
        assert_eq!(name_in(Some(&s), Role::Art), "Art screen");
        assert!(has_in(Some(&s), Role::Display));
        // No screens at all: no cards; nothing known: the info and art cards stay.
        assert!(!has_in(Some(&[]), Role::Info) && !has_in(Some(&[]), Role::Art));
        assert!(has_in(None, Role::Art) && !has_in(None, Role::Display));
    }

    #[test]
    fn a_profile_names_its_screens_and_the_firmware_sizes_them() {
        let mut b = users_desk::board();
        b.screens[1].name = Some("Fox screen".into());
        let display = |id, parent, kind, w, h, rotation| ScreenDescriptor {
            count: 4,
            index: id,
            id,
            kind,
            half: 0,
            parent,
            width: w,
            height: h,
            x: 0,
            y: 0,
            rotation,
            format: 0,
            ops: 0,
            stride: 0,
            rows_per_report: 0,
            readback_half: None,
            version: 1,
        };
        // Two displays, each holding a region: the regions are the screens.
        let described = [
            display(0, None, ScreenKind::Display, 160, 68, 0),
            display(1, Some(0), ScreenKind::Panel, 64, 52, 0),
            display(2, None, ScreenKind::Display, 160, 68, 0),
            display(3, Some(2), ScreenKind::Art, 140, 68, 1),
        ];
        let s = of_descriptors(&described, Some(&b));
        let named: Vec<_> = s.iter().map(|s| (s.id, s.name.as_str())).collect();
        assert_eq!(named, [(1, "Info screen"), (3, "Fox screen")]);
        assert_eq!(s[1].upright(), (68, 140));
        // No profile: the roles' names.
        assert_eq!(of_descriptors(&described, None)[1].name, "Art screen");
    }
}
