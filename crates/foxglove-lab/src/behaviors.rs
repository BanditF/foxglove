//! Offline Studio behavior metadata. FXK1 format follows Foxglove's
//! `app/src/foxglove/keymap_cache.rs`; protobuf tags follow ZMK Studio behaviors.proto.
//! Only the required protobuf fields are decoded, without linking the app.
use anyhow::{ensure, Context, Result};
use prost::Message;
use std::{collections::BTreeMap, path::PathBuf};

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
    pub fn parse(mut bytes: &[u8]) -> Result<Self> {
        ensure!(
            take(&mut bytes, 4)? == b"FXK1",
            "not an FXK1 Studio recording"
        );
        let count = number(&mut bytes)?;
        ensure!(count <= 10000, "oversized Studio cache");
        let mut c = Self::default();
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
                let id = u16::try_from(d.id)?;
                let name = match d.name.as_str() {
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
                .to_string();
                if let Some(old) = c.ids.insert(name.clone(), id) {
                    ensure!(old == id, "ambiguous behavior {name}");
                }
            }
        }
        ensure!(bytes.is_empty(), "trailing Studio cache bytes");
        Ok(c)
    }

    /// Uses an explicit cache, or a single unambiguous cache in APPDATA.
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = match path {
            Some(p) => p,
            None => {
                let dir = PathBuf::from(
                    std::env::var("APPDATA").context("APPDATA unavailable; use --keymap")?,
                )
                .join("Foxglove/config");
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
            "{} (offline cache; verify it matches flashed firmware)",
            path.display()
        );
        Ok(c)
    }
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
        assert!(Catalog::parse(&b[..b.len() - 1]).is_err());
        b.push(0);
        assert!(Catalog::parse(&b).is_err());
    }
}
