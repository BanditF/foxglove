//! A keyboard's screens by role (#560): what each screen is for and what the app calls it,
//! never which side it sits on. The firmware's own descriptors (HELLO capability SCREENS) win;
//! the board profile's `[[screens]]` fill in what the firmware doesn't say, and give each screen
//! its name (`name`, else the role's: "Info screen" for the panel the PC draws, "Art screen" for
//! where art plays, "Display" for one the firmware draws itself).
//!
//! Every string that names a screen comes from here, so a board with one screen, or three of
//! different sizes, reads right.

use foxglove_link::protocol::{ScreenDescriptor, ScreenKind};

use super::boards::{Board, Refresh};

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
    /// How it redraws (#833): the profile's `refresh`, `Fast` when it doesn't say.
    pub refresh: Refresh,
    /// The shortest time between two frames, in ms (#833); 0 is no limit.
    pub min_frame_ms: u32,
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
                refresh: s.refresh,
                min_frame_ms: s.min_frame_ms,
            })
        })
        .collect()
}

/// The screens of the board Foxglove's older firmware was built for (#563,
/// [`super::boards::old_firmware_board`]): what a keyboard that describes none, and has no
/// profile, is taken to have. Empty only if no built-in profile says it is that board.
pub fn old_firmware() -> Vec<Screen> {
    super::boards::old_firmware_board()
        .map(of_board)
        .unwrap_or_default()
}

/// The upright size of the older firmware's screen of `role`, for a keyboard that doesn't say.
pub fn old_firmware_size(role: Role) -> Option<(u16, u16)> {
    first(&old_firmware(), role).map(Screen::upright)
}

/// How an art screen stores its picture, from its upright size as the firmware describes it
/// (`screens_corne.c`: "upright as the art tools draw it, stored turned in the NVA1 blobs"): the
/// profile's art screen of that upright size, else the older firmware's art screen when it is
/// that size, else as it is (upright, unturned).
fn art_stored(upright: (u16, u16), profile: Option<&Screen>) -> (u16, u16, u8) {
    art_stored_with(upright, profile, &old_firmware())
}

/// [`art_stored`] with `old` as the older firmware's screens.
fn art_stored_with(
    upright: (u16, u16),
    profile: Option<&Screen>,
    old: &[Screen],
) -> (u16, u16, u8) {
    if let Some(p) = profile.filter(|p| p.upright() == upright) {
        return (p.width, p.height, p.rotation);
    }
    if let Some(f) = first(old, Role::Art).filter(|f| f.upright() == upright) {
        return (f.width, f.height, f.rotation);
    }
    (upright.0, upright.1, 0)
}

/// The screens the firmware describes, named from the profile (the n-th screen of a role takes
/// the profile's n-th of that role). A display that holds panels or art areas isn't one of its
/// own: its regions are. A panel is sized as the PC addresses it; an art area is described
/// upright, and stored as [`art_stored`] says.
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
            let profile = named.iter().filter(|s| s.role == role).nth(nth);
            let name = profile.map_or_else(|| role.default_name().to_string(), |s| s.name.clone());
            let (width, height, rotation) = match role {
                Role::Art => art_stored((d.width, d.height), profile),
                Role::Info => (d.width, d.height, 0),
                Role::Display => (d.width, d.height, d.rotation % 4),
            };
            Some(Screen {
                id: d.id,
                role,
                name,
                width,
                height,
                format: if d.format == ScreenDescriptor::FORMAT_1BIT {
                    "1bit".to_string()
                } else {
                    d.format_name().to_string()
                },
                rotation,
                // The descriptor doesn't carry them yet (#833): the profile's screen of that
                // role says, else a fast screen with no limit.
                refresh: profile.map_or(Refresh::Fast, |s| s.refresh),
                min_frame_ms: profile.map_or(0, |s| s.min_frame_ms),
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

    /// #563: the screens of firmware that describes none come from the older firmware's
    /// profile: the Corne's give the 64 x 52 panel and the 140 x 68 turned art screen the code
    /// had; a made-up board's screens stand in the same way.
    #[test]
    fn the_old_firmwares_screens_come_from_its_profile() {
        use crate::foxglove::art_encode::ArtTarget;
        assert_eq!(old_firmware_size(Role::Info), Some((64, 52)));
        assert_eq!(old_firmware_size(Role::Art), Some((68, 140)));
        let old = old_firmware();
        let art = first(&old, Role::Art).unwrap();
        assert_eq!(ArtTarget::of(art), Some(ArtTarget::FIRMWARE));
        // No profile, nothing described: the art target is the one it always was.
        assert_eq!(ArtTarget::current(), ArtTarget::FIRMWARE);
        // A described upright 68 x 140 art screen is stored turned, as before.
        assert_eq!(art_stored((68, 140), None), (140, 68, 1));
        assert_eq!(art_stored((80, 160), None), (80, 160, 0));

        // Plank 3 as the older firmware's board: its 160 x 80 art screen is the one stored as
        // it is, and the Corne's shape is just another size.
        let plank = of_board(&second_desk::board());
        assert_eq!(art_stored_with((160, 80), None, &plank), (160, 80, 0));
        assert_eq!(art_stored_with((68, 140), None, &plank), (68, 140, 0));
        assert_eq!(
            first(&plank, Role::Info).map(Screen::upright),
            Some((128, 32))
        );
    }

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
        // The Corne's own descriptors (screens_corne.c): two displays, each holding a region;
        // the art region described upright. The regions are the screens.
        let described = [
            display(0, None, ScreenKind::Display, 160, 68, 1),
            display(1, Some(0), ScreenKind::Panel, 64, 52, 0),
            display(2, None, ScreenKind::Display, 160, 68, 1),
            display(3, Some(2), ScreenKind::Art, 68, 140, 0),
        ];
        let s = of_descriptors(&described, Some(&b));
        let named: Vec<_> = s.iter().map(|s| (s.id, s.name.as_str())).collect();
        assert_eq!(named, [(1, "Info screen"), (3, "Fox screen")]);
        // Stored as the firmware's blobs are: 140x68, turned; the same as the profile says.
        assert_eq!((s[0].width, s[0].height, s[0].rotation), (64, 52, 0));
        assert_eq!((s[1].width, s[1].height, s[1].rotation), (140, 68, 1));
        assert_eq!(s[1].upright(), (68, 140));
        assert_eq!(s, {
            let mut p = of_board(&b);
            p[0].id = 1;
            p[1].id = 3;
            p
        });
        // No profile: the roles' names, and the firmware's art format all the same.
        let bare = of_descriptors(&described, None);
        assert_eq!(bare[1].name, "Art screen");
        assert_eq!(
            (bare[1].width, bare[1].height, bare[1].rotation),
            (140, 68, 1)
        );
        // An upright art area of another size, with nothing to say otherwise: stored upright.
        let wide = [display(5, None, ScreenKind::Art, 160, 80, 0)];
        let w = &of_descriptors(&wide, None)[0];
        assert_eq!((w.width, w.height, w.rotation), (160, 80, 0));
    }
}
