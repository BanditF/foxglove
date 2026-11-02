//! Studio behavior metadata: the app's saved keymap (FXK1/FXK2, as Foxglove's
//! `app/src/foxglove/keymap_cache.rs` writes it; protobuf tags follow ZMK Studio behaviors.proto,
//! only the required fields are decoded, without linking the app), or the lab's own Studio read
//! ([`crate::studio`]).
//!
//! Behavior ids belong to one firmware build (#633): a flash can renumber them. The saved copy
//! is used only when it names the build the left half runs now (D7 09); otherwise, and when it
//! is missing or empty, [`resolve`] asks the running app's `behaviors.list` (#639), then reads
//! the ids from the keyboard (#618).
use anyhow::{ensure, Context, Result};
use prost::Message;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, PartialEq, Message)]
struct Response {
    #[prost(message, optional, tag = "1")]
    reply: Option<Reply>,
}
#[derive(Clone, PartialEq, Message)]
struct Reply {
    #[prost(message, optional, tag = "4")]
    behaviors: Option<Behaviors>,
}
#[derive(Clone, PartialEq, Message)]
struct Behaviors {
    #[prost(message, optional, tag = "2")]
    details: Option<Details>,
}
#[derive(Clone, PartialEq, Message)]
struct Details {
    #[prost(uint32, tag = "1")]
    id: u32,
    #[prost(string, tag = "2")]
    name: String,
}

#[derive(Default, Debug)]
pub struct Catalog {
    pub ids: BTreeMap<String, u16>,
    pub source: String,
    /// The firmware build a saved copy was recorded from (FXK2); `None` for FXK1 copies, copies
    /// whose build wasn't known, and catalogs not read from a file.
    pub build: Option<String>,
}

/// A build as the app keys it (`devices::build_key`): `major.minor.patch-profile-sha`.
pub fn build_key(b: &foxglove_link::protocol::BuildInfo) -> String {
    let v = &b.version;
    format!(
        "{}.{}.{}-{}-{}",
        v.major, v.minor, v.patch, v.profile, b.sha
    )
}

/// The lab's short name for a Studio display name.
pub fn short_name(display: &str) -> &str {
    match display {
        "Key Press" | "key_press" => "kp",
        "Momentary Layer" | "momentary_layer" => "mo",
        "Toggle Layer" | "toggle_layer" => "tog",
        "To Layer" => "to",
        "Sticky Layer" => "sl",
        "Underglow" => "rgb_ug",
        "Backlight" => "bl",
        "External Power" => "ext_power",
        "None" => "none",
        "Transparent" => "trans",
        "Bluetooth" => "bt",
        "Output Selection" => "out",
        "Reset" => "sys_reset",
        "Soft Off" | "z_so_off" => "soft_off",
        "Bootloader" => "bootloader",
        other => other,
    }
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    ensure!(bytes.len() >= n, "truncated Studio cache");
    let (head, tail) = bytes.split_at(n);
    *bytes = tail;
    Ok(head)
}
fn number(bytes: &mut &[u8]) -> Result<usize> {
    Ok(u32::from_le_bytes(take(bytes, 4)?.try_into()?) as usize)
}

impl Catalog {
    /// Ids from (id, Studio display name) pairs; one name with two ids is an error.
    pub fn from_details(details: impl IntoIterator<Item = (u32, String)>) -> Result<Self> {
        let mut c = Self::default();
        for (id, name) in details {
            let id = u16::try_from(id)?;
            let name = short_name(&name).to_string();
            if let Some(old) = c.ids.insert(name.clone(), id) {
                ensure!(old == id, "ambiguous behavior {name}");
            }
        }
        Ok(c)
    }

    /// The running app's `behaviors.list` (#639): each listed action's kind (API 1.9's
    /// `behavior`; an older app gives none) and id. Only what the app may invoke is listed, so
    /// `kp`, `mo` and the refused `&bt`, `&out`, `&sys_reset`... are never in it.
    pub fn from_app(list: &foxglove_plugin_api::methods::Behaviors) -> Result<Self> {
        let mut c = Self {
            source: "the app's behaviors.list".into(),
            ..Default::default()
        };
        for b in &list.behaviors {
            let Some(name) = b.behavior.as_deref() else {
                continue;
            };
            let id = u16::try_from(b.id)?;
            if let Some(old) = c.ids.insert(name.to_string(), id) {
                ensure!(old == id, "ambiguous behavior {name} in the app's list");
            }
        }
        Ok(c)
    }

    /// A saved copy: "FXK2", the build (u32 LE length, 0 for not known, then UTF-8), the
    /// exchanges' count, then each request and reply (u32 LE length, protobuf). "FXK1" copies
    /// (before #633) have no build.
    pub fn parse(mut bytes: &[u8]) -> Result<Self> {
        let build = match take(&mut bytes, 4)? {
            b"FXK2" => {
                let n = number(&mut bytes)?;
                ensure!(n <= 256, "oversized build in Studio cache");
                let b = std::str::from_utf8(take(&mut bytes, n)?)?.to_string();
                Some(b).filter(|b| !b.is_empty())
            }
            b"FXK1" => None,
            _ => anyhow::bail!("not an FXK1/FXK2 Studio recording"),
        };
        let count = number(&mut bytes)?;
        ensure!(count <= 10000, "oversized Studio cache");
        let mut details = Vec::new();
        for _ in 0..count {
            let n = number(&mut bytes)?;
            take(&mut bytes, n)?; // request
            let n = number(&mut bytes)?;
            let response = Response::decode(take(&mut bytes, n)?)?;
            if let Some(d) = response
                .reply
                .and_then(|r| r.behaviors)
                .and_then(|b| b.details)
            {
                details.push((d.id, d.name));
            }
        }
        ensure!(bytes.is_empty(), "trailing Studio cache bytes");
        let mut c = Self::from_details(details)?;
        c.build = build;
        Ok(c)
    }

    /// Why this saved copy's ids can't be used while the left runs `left_build` (its D7 09
    /// build), or `None` when they can: the copy must name that very build and hold ids.
    pub fn untrusted(&self, left_build: Option<&str>) -> Option<String> {
        let Some(left) = left_build else {
            return Some(
                "the left's build (D7 09) couldn't be read, so no saved ids are trusted".into(),
            );
        };
        match self.build.as_deref() {
            None => Some("the saved keymap names no build (FXK1, or not known)".into()),
            Some(b) if b != left => Some(format!(
                "the saved keymap is from build {b}, the left runs {left}"
            )),
            Some(_) if self.ids.is_empty() => Some("the saved keymap has no behaviors".into()),
            Some(_) => None,
        }
    }

    /// Uses an explicit cache, or a single unambiguous cache in FOXGLOVE_CONFIG_DIR or APPDATA
    /// (read only).
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = match path {
            Some(p) => p,
            None => {
                // A test config first; otherwise the user's real one, read only.
                let dir = match std::env::var_os("FOXGLOVE_CONFIG_DIR") {
                    Some(d) => PathBuf::from(d),
                    None => PathBuf::from(
                        std::env::var("APPDATA").context("APPDATA unavailable; use --keymap")?,
                    )
                    .join("Foxglove/config"),
                };
                let files: Vec<_> = std::fs::read_dir(dir)?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.extension().is_some_and(|x| x == "studio")
                            && p.file_name()
                                .is_some_and(|n| n.to_string_lossy().starts_with("keymap-"))
                    })
                    .collect();
                ensure!(
                    files.len() == 1,
                    "need exactly one cached keymap; select with --keymap PATH"
                );
                files[0].clone()
            }
        };
        let mut c = Self::parse(&std::fs::read(&path)?)?;
        c.source = format!(
            "{} (saved keymap, build {})",
            path.display(),
            c.build.as_deref().unwrap_or("not recorded")
        );
        Ok(c)
    }
}

/// The ids for firmware whose left half runs `left_build`: the saved copy when it names that
/// build and holds ids; else the running app's list (`app`, `None` while the app isn't up) when
/// it has every behavior in `needed`; else the keyboard's own answer from `read` (the lab's
/// Studio read), with the app's ids for anything the read lacks. With none of them, the app's
/// partial list, or an empty catalog, whose source says why, so missing names fail safely.
pub fn resolve(
    cache: Result<Catalog>,
    left_build: Option<&str>,
    app: Option<Result<Catalog>>,
    needed: &BTreeSet<String>,
    read: impl FnOnce() -> Result<Catalog>,
) -> Catalog {
    let mut why = match cache {
        Ok(mut c) => match c.untrusted(left_build) {
            None => {
                c.source = format!("{}; matches the left's build", c.source);
                return c;
            }
            Some(why) => format!("{}: {why}", c.source),
        },
        Err(e) => format!("no saved keymap: {e:#}"),
    };
    let mut partial = None;
    match app {
        None => {}
        Some(Ok(mut c)) if !c.ids.is_empty() => {
            let missing: Vec<&str> = needed
                .iter()
                .map(String::as_str)
                .filter(|n| !c.ids.contains_key(*n))
                .collect();
            if missing.is_empty() {
                c.source = format!("{} (not the saved keymap: {why})", c.source);
                return c;
            }
            why = format!("{why}; the app's list lacks {}", missing.join(", "));
            partial = Some(c);
        }
        Some(Ok(c)) => why = format!("{why}; {} listed no behaviors", c.source),
        Some(Err(e)) => why = format!("{why}; the app's behaviors.list: {e:#}"),
    }
    let read = read();
    let read_ok = matches!(&read, Ok(c) if !c.ids.is_empty());
    if let Some(mut p) = partial.take_if(|_| !read_ok) {
        let failed = match read {
            Ok(c) => format!("{} listed no behaviors", c.source),
            Err(e) => format!("the Studio read failed: {e:#}"),
        };
        p.source = format!(
            "{} (not the saved keymap: {why}; {failed}; missing names fail safely)",
            p.source
        );
        return p;
    }
    match read {
        Ok(mut c) if !c.ids.is_empty() => {
            if let Some(p) = partial {
                for (name, id) in p.ids {
                    c.ids.entry(name).or_insert(id);
                }
            }
            c.source = format!("{} (not the saved keymap: {why})", c.source);
            c
        }
        Ok(c) => Catalog {
            source: format!(
                "{} listed no behaviors; {why}; named invokes will fail safely",
                c.source
            ),
            ..Default::default()
        },
        Err(e) => Catalog {
            source: format!(
                "No trusted behavior ids: {why}; the Studio read failed: {e:#}; named invokes \
                 will fail safely"
            ),
            ..Default::default()
        },
    }
}

/// The behaviors a run needs ids for (#639): every named invoke in `cases`, wherever it sits
/// (a step, a hold's `while_down`), plus `rgb_ug` for restoring the lights afterwards.
pub fn needed_names(cases: &[crate::spec::Case]) -> BTreeSet<String> {
    fn walk(v: &serde_json::Value, out: &mut BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    match (k.as_str(), v) {
                        ("behavior", serde_json::Value::String(s)) => {
                            out.insert(s.trim_start_matches('&').to_string());
                        }
                        _ => walk(v, out),
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::from(["rgb_ug".to_string()]);
    for c in cases {
        if let Ok(v) = serde_json::to_value(&c.steps) {
            walk(&v, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_decodes_ids_and_rejects_damage() {
        let r = Response {
            reply: Some(Reply {
                behaviors: Some(Behaviors {
                    details: Some(Details {
                        id: 42,
                        name: "Momentary Layer".into(),
                    }),
                }),
            }),
        }
        .encode_to_vec();
        let mut b = b"FXK1".to_vec();
        for n in [1u32, 0, r.len() as u32] {
            b.extend(n.to_le_bytes());
        }
        b.extend(r);
        assert_eq!(Catalog::parse(&b).unwrap().ids["mo"], 42);
        assert_eq!(Catalog::parse(&b).unwrap().build, None);
        assert!(Catalog::parse(&b[..b.len() - 1]).is_err());
        b.push(0);
        assert!(Catalog::parse(&b).is_err());
    }

    /// FXK2 as the app writes it since #636: the build, then the exchanges.
    fn fxk2(build: &str) -> Vec<u8> {
        let r = Response {
            reply: Some(Reply {
                behaviors: Some(Behaviors {
                    details: Some(Details {
                        id: 7,
                        name: "Momentary Layer".into(),
                    }),
                }),
            }),
        }
        .encode_to_vec();
        let mut b = b"FXK2".to_vec();
        b.extend((build.len() as u32).to_le_bytes());
        b.extend(build.as_bytes());
        for n in [1u32, 0, r.len() as u32] {
            b.extend(n.to_le_bytes());
        }
        b.extend(r);
        b
    }

    #[test]
    fn fxk2_records_its_build() {
        let c = Catalog::parse(&fxk2("0.1.0-0-13ff04b")).unwrap();
        assert_eq!(
            (c.ids["mo"], c.build.as_deref()),
            (7, Some("0.1.0-0-13ff04b"))
        );
        assert_eq!(Catalog::parse(&fxk2("")).unwrap().build, None);
        assert!(Catalog::parse(&fxk2("x")[..10]).is_err());
    }

    fn studio() -> Result<Catalog> {
        let mut c = Catalog::from_details([(9, "Momentary Layer".to_string())])?;
        c.source = "Studio read of COM9".into();
        Ok(c)
    }

    /// #618: the saved copy is used only for the build the left runs; otherwise the lab reads.
    #[test]
    fn resolve_trusts_only_a_matching_nonempty_build() {
        let left = Some("0.1.0-0-13ff04b");
        let same = || Catalog::parse(&fxk2("0.1.0-0-13ff04b"));
        let none = BTreeSet::new();
        let c = resolve(same(), left, None, &none, || {
            panic!("no read for a matching cache")
        });
        assert_eq!(c.ids["mo"], 7);
        assert!(
            c.source.contains("matches the left's build"),
            "{}",
            c.source
        );
        // Another build, no build (FXK1), the left's build unknown, an empty copy, no copy: read.
        let cases: [(Result<Catalog>, Option<&str>, &str); 5] = [
            (
                Catalog::parse(&fxk2("0.1.0-0-88e3a59")),
                left,
                "is from build 0.1.0-0-88e3a59",
            ),
            (
                Ok(Catalog {
                    ids: [("mo".to_string(), 7)].into(),
                    ..Default::default()
                }),
                left,
                "names no build",
            ),
            (same(), None, "couldn't be read"),
            (
                Ok(Catalog {
                    build: left.map(String::from),
                    ..Default::default()
                }),
                left,
                "has no behaviors",
            ),
            (
                Err(anyhow::anyhow!("need exactly one cached keymap")),
                left,
                "no saved keymap",
            ),
        ];
        for (cache, build, why) in cases {
            let c = resolve(cache, build, None, &none, studio);
            assert_eq!(c.ids["mo"], 9, "{why}");
            assert!(
                c.source.contains("COM9") && c.source.contains(why),
                "{}",
                c.source
            );
        }
        // The read failed too: empty, saying both reasons.
        let c = resolve(Catalog::parse(&fxk2("other")), left, None, &none, || {
            anyhow::bail!("port busy")
        });
        assert!(c.ids.is_empty());
        assert!(
            c.source.contains("other") && c.source.contains("port busy"),
            "{}",
            c.source
        );
    }

    fn app_list(kinds: &[(&str, u32)]) -> Result<Catalog> {
        use foxglove_plugin_api::methods::{Behavior, Behaviors};
        Catalog::from_app(&Behaviors {
            behaviors: kinds
                .iter()
                .map(|&(k, id)| Behavior {
                    id,
                    param1: 1,
                    param2: 0,
                    name: "Lights: On".into(),
                    needs_confirmation: false,
                    behavior: Some(k.to_string()),
                })
                .collect(),
        })
    }

    /// #639: with the app up, its `behaviors.list` comes before the Studio read; Studio is read
    /// only for names the app doesn't list, and the app's ids stand when that read fails.
    #[test]
    fn resolve_prefers_the_apps_list_then_falls_back_to_studio() {
        let left = Some("0.1.0-0-13ff04b");
        let other = || Catalog::parse(&fxk2("0.1.0-0-88e3a59"));
        let needs = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<BTreeSet<_>>();
        // Everything needed is listed: no Studio read.
        let app = Some(app_list(&[("rgb_ug", 4)]));
        let c = resolve(other(), left, app, &needs(&["rgb_ug"]), || {
            panic!("no Studio read when the app lists it")
        });
        assert_eq!(c.ids["rgb_ug"], 4);
        assert!(
            c.source.contains("behaviors.list") && c.source.contains("88e3a59"),
            "{}",
            c.source
        );
        // `mo` isn't listed (the app never offers it): Studio fills it, the app's ids stay.
        let app = Some(app_list(&[("rgb_ug", 4)]));
        let c = resolve(other(), left, app, &needs(&["rgb_ug", "mo"]), studio);
        assert_eq!((c.ids["mo"], c.ids["rgb_ug"]), (9, 4));
        assert!(
            c.source.contains("COM9") && c.source.contains("lacks mo"),
            "{}",
            c.source
        );
        // The Studio read fails (the app holds the port): the app's list, saying what's missing.
        let app = Some(app_list(&[("rgb_ug", 4)]));
        let c = resolve(other(), left, app, &needs(&["mo"]), || {
            anyhow::bail!("port busy")
        });
        assert_eq!(c.ids.len(), 1);
        assert!(
            c.source.contains("lacks mo") && c.source.contains("port busy"),
            "{}",
            c.source
        );
        // Not known yet, or an older app without kinds: Studio as before.
        for app in [
            Err(anyhow::anyhow!("aren't known yet (code 3)")),
            app_list(&[]),
        ] {
            let c = resolve(other(), left, Some(app), &needs(&["rgb_ug"]), studio);
            assert_eq!(c.ids["mo"], 9);
            assert!(c.source.contains("COM9"), "{}", c.source);
        }
        // Two ids for one kind is refused.
        assert!(app_list(&[("tog", 1), ("tog", 2)]).is_err());
    }

    #[test]
    fn needed_names_come_from_every_invoke() {
        let suite: crate::spec::Suite = toml::from_str(
            r#"
schema = 1

[[cases]]
name = "a"
description = "a"
steps = [{ invoke = { behavior = "&mo", param1 = 1 } }, { invoke = { local_id = 3 } }]
"#,
        )
        .unwrap();
        let n = needed_names(&suite.cases);
        assert_eq!(n.into_iter().collect::<Vec<_>>(), ["mo", "rgb_ug"]);
    }
}
