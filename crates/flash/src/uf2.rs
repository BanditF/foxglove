//! UF2 checks before anything touches a drive, the same as `scripts/flash/foxglove_flash.py`'s
//! `preflight`: whole valid nRF52840 blocks, payloads inside the application area, and the
//! half the image is for, from its marker (`FOXGLOVE_HALF=left;VER=..;PROFILE=..;SHA=..`) or the
//! two feature strings each half's build always has.

use crate::Half;

/// Where the application area starts (every inspected build, settings reset included).
pub const START: u32 = 0x26000;
/// The bootloader's limit: an image may not reach past it.
pub const LIMIT: u32 = 0xAD000;

const MAGIC: (u32, u32, u32) = (0x0A32_4655, 0x9E5D_5157, 0x0AB1_6F30);
const FAMILY_FLAG: u32 = 0x2000;
const NRF52840: u32 = 0xADA5_2840;

/// Stable strings in each half's build, for images without the explicit marker.
const LEFT_MARKERS: [&[u8]; 2] = [b"studio_rpc_thread", b"raw_hid_received_event"];
const RIGHT_MARKERS: [&[u8]; 2] = [
    b"Split Peripheral Notification Queue",
    b"ble_peripheral\x00",
];

/// What an image is for, and whether it may be written as `half`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    pub reasons: Vec<String>,
    /// The highest address the image writes.
    pub end: u32,
    pub detected: Option<Half>,
    /// The settings-reset image: never written by an update.
    pub settings_reset: bool,
    pub version: String,
    /// "dev" or "ga", when the marker says.
    pub profile: String,
    pub sha: String,
}

impl Preflight {
    pub fn ok(&self) -> bool {
        self.reasons.is_empty()
    }
    /// Free space under the bootloader's limit, in KiB.
    pub fn headroom_kib(&self) -> f32 {
        (LIMIT.saturating_sub(self.end)) as f32 / 1024.0
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The explicit markers in the flat image: (half, version, profile, sha) each.
fn markers(image: &[u8]) -> Vec<(Half, String, String, String)> {
    const KEY: &[u8] = b"FOXGLOVE_HALF=";
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = image[at..].windows(KEY.len()).position(|w| w == KEY) {
        let start = at + i + KEY.len();
        at = start;
        let rest = &image[start..];
        let half = if rest.starts_with(b"left") {
            Half::Left
        } else if rest.starts_with(b"right") {
            Half::Right
        } else {
            continue;
        };
        let mut pos = half.name().len();
        let mut field = |key: &[u8], ok: &dyn Fn(u8) -> bool, max: usize| -> String {
            if !rest[pos..].starts_with(key) {
                return String::new();
            }
            let value: Vec<u8> = rest[pos + key.len()..]
                .iter()
                .copied()
                .take_while(|b| ok(*b))
                .take(max)
                .collect();
            if value.is_empty() {
                return String::new();
            }
            pos += key.len() + value.len();
            String::from_utf8_lossy(&value).into_owned()
        };
        let version = field(
            b";VER=",
            &|b| b.is_ascii_alphanumeric() || b".+-".contains(&b),
            32,
        );
        let profile = field(b";PROFILE=", &|b| b.is_ascii_lowercase(), 3);
        let profile = if profile == "dev" || profile == "ga" {
            profile
        } else {
            String::new()
        };
        let sha = field(
            b";SHA=",
            &|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase(),
            40,
        );
        let sha = if sha.len() >= 7 { sha } else { String::new() };
        out.push((half, version, profile, sha));
    }
    out
}

/// Checks `data` as the image for `half`. Nothing in `reasons` means it may be written.
pub fn preflight(data: &[u8], half: Half) -> Preflight {
    let mut p = Preflight::default();
    if data.is_empty() || !data.len().is_multiple_of(512) {
        p.reasons.push("a UF2 must be whole 512-byte blocks".into());
        return p;
    }
    let count = data.len() / 512;
    let mut numbers = Vec::with_capacity(count);
    let mut image = vec![0xffu8; (LIMIT - START) as usize];
    let mut ranges = Vec::new();
    for (index, block) in data.as_chunks::<512>().0.iter().enumerate() {
        let (m0, m1, flags, addr, size, number, total, family) = (
            u32_at(block, 0),
            u32_at(block, 4),
            u32_at(block, 8),
            u32_at(block, 12),
            u32_at(block, 16),
            u32_at(block, 20),
            u32_at(block, 24),
            u32_at(block, 28),
        );
        let label = format!("block {index}");
        if (m0, m1, u32_at(block, 508)) != MAGIC {
            p.reasons.push(format!("{label}: bad magic"));
        }
        if flags & FAMILY_FLAG == 0 || family != NRF52840 {
            p.reasons.push(format!("{label}: not an nRF52840 image"));
        }
        if flags & !FAMILY_FLAG != 0 {
            p.reasons.push(format!("{label}: unsupported UF2 flags"));
        }
        if total as usize != count {
            p.reasons.push(format!("{label}: inconsistent block count"));
        }
        numbers.push(number);
        p.end = p.end.max(addr.saturating_add(size));
        if size == 0 || size > 476 {
            p.reasons
                .push(format!("{label}: invalid payload size {size}"));
        }
        if addr < START || addr.saturating_add(size) > LIMIT {
            p.reasons
                .push(format!("{label}: address outside 0x{START:X}..0x{LIMIT:X}"));
        } else if size > 0 && size <= 476 {
            ranges.push((addr, addr + size));
            let at = (addr - START) as usize;
            image[at..at + size as usize].copy_from_slice(&block[32..32 + size as usize]);
        }
    }
    numbers.sort_unstable();
    if numbers.iter().enumerate().any(|(i, n)| *n as usize != i) {
        p.reasons
            .push("block numbers must be exactly 0..count-1".into());
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|w| w[0].1 > w[1].0) {
        p.reasons.push("overlapping flash payloads".into());
    }
    // In the flat image, so a marker split across blocks is still found.
    let marks = markers(&image);
    if contains(&image, b"SETTINGS RESET") {
        p.settings_reset = true;
        p.reasons.push("this is the settings-reset image, which erases settings and pairings; an update never writes it".into());
    } else if !marks.is_empty() {
        let halves: Vec<Half> = marks.iter().map(|m| m.0).collect();
        if halves.iter().all(|h| *h == halves[0]) {
            p.detected = Some(halves[0]);
            if halves[0] != half {
                p.reasons.push(format!(
                    "the image is for the {} half, not the {}",
                    halves[0].name(),
                    half.name()
                ));
            }
        } else {
            p.reasons.push("conflicting half markers".into());
        }
    } else {
        let left = LEFT_MARKERS.iter().all(|m| contains(&image, m));
        let right = RIGHT_MARKERS.iter().all(|m| contains(&image, m));
        match (left, right) {
            (true, false) => p.detected = Some(Half::Left),
            (false, true) => p.detected = Some(Half::Right),
            _ => p
                .reasons
                .push("can't tell which half this image is for".into()),
        }
        if p.detected.is_some_and(|d| d != half) {
            p.reasons.push(format!(
                "the image is for the {} half, not the {}",
                p.detected.unwrap().name(),
                half.name()
            ));
        }
    }
    if let [(_, version, profile, sha)] = marks.as_slice() {
        (p.version, p.profile, p.sha) = (version.clone(), profile.clone(), sha.clone());
    }
    p
}

/// Checks `data` as the image for a one-piece keyboard (#559): the same checks, but it needs no
/// half marker. One for a split's right half is still refused.
pub fn preflight_one(data: &[u8]) -> Preflight {
    let mut p = preflight(data, Half::Left);
    p.reasons.retain(|r| {
        r != "can't tell which half this image is for" && !r.starts_with("the image is for the ")
    });
    if p.detected == Some(Half::Right) {
        p.reasons
            .push("the image is for a split keyboard's right half".into());
    }
    p
}

/// A UF2 holding `payload` from [`START`], in 256-byte blocks, for tests.
pub fn build(payload: &[u8]) -> Vec<u8> {
    let chunks: Vec<&[u8]> = payload.chunks(256).collect();
    let mut out = Vec::new();
    for (n, chunk) in chunks.iter().enumerate() {
        let mut block = vec![0u8; 512];
        for (at, v) in [
            (0, MAGIC.0),
            (4, MAGIC.1),
            (8, FAMILY_FLAG),
            (12, START + 256 * n as u32),
            (16, 256),
            (20, n as u32),
            (24, chunks.len() as u32),
            (28, NRF52840),
            (508, MAGIC.2),
        ] {
            block[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        block[32..32 + chunk.len()].copy_from_slice(chunk);
        out.extend(block);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(text: &str) -> Vec<u8> {
        let mut payload = vec![0u8; 1024];
        payload.extend(text.as_bytes());
        payload.resize(payload.len().div_ceil(256) * 256, 0);
        build(&payload)
    }

    #[test]
    fn a_marked_image_passes_for_its_half_only() {
        let left = image("FOXGLOVE_HALF=left;VER=0.2.0;PROFILE=dev;SHA=abc1234");
        let p = preflight(&left, Half::Left);
        assert!(p.ok(), "{:?}", p.reasons);
        assert_eq!(
            (
                p.detected,
                p.version.as_str(),
                p.profile.as_str(),
                p.sha.as_str()
            ),
            (Some(Half::Left), "0.2.0", "dev", "abc1234")
        );
        assert!(p.headroom_kib() > 400.0);
        let wrong = preflight(&left, Half::Right);
        assert!(!wrong.ok());
        assert!(wrong.reasons[0].contains("for the left half"));
    }

    #[test]
    fn a_marker_split_across_blocks_is_found() {
        let mut payload = vec![0u8; 250];
        payload.extend(b"FOXGLOVE_HALF=right;VER=0.2.0;PROFILE=ga;SHA=1234567");
        payload.resize(512, 0);
        let p = preflight(&build(&payload), Half::Right);
        assert!(p.ok(), "{:?}", p.reasons);
        assert_eq!(p.profile, "ga");
    }

    #[test]
    fn feature_strings_name_the_half_when_there_is_no_marker() {
        let right = image("Split Peripheral Notification Queue ble_peripheral\0");
        assert_eq!(preflight(&right, Half::Right).detected, Some(Half::Right));
        assert!(preflight(&right, Half::Left)
            .reasons
            .iter()
            .any(|r| r.contains("for the right half")));
        assert!(
            !preflight(&image("nothing to see"), Half::Left).ok(),
            "unknown: refused"
        );
    }

    #[test]
    fn the_settings_reset_image_is_never_an_update() {
        let p = preflight(&image("SETTINGS RESET"), Half::Left);
        assert!(p.settings_reset && !p.ok());
    }

    #[test]
    fn broken_blocks_are_refused() {
        let good = image("FOXGLOVE_HALF=left");
        assert!(!preflight(&good[..500], Half::Left).ok(), "partial block");
        let mut magic = good.clone();
        magic[0] ^= 1;
        assert!(preflight(&magic, Half::Left)
            .reasons
            .iter()
            .any(|r| r.contains("bad magic")));
        let mut family = good.clone();
        family[28] = 0;
        assert!(preflight(&family, Half::Left)
            .reasons
            .iter()
            .any(|r| r.contains("nRF52840")));
        let mut addr = good.clone();
        addr[12..16].copy_from_slice(&LIMIT.to_le_bytes());
        assert!(preflight(&addr, Half::Left)
            .reasons
            .iter()
            .any(|r| r.contains("address outside")));
        let mut number = good.clone();
        number[20] = 9;
        assert!(preflight(&number, Half::Left)
            .reasons
            .iter()
            .any(|r| r.contains("block numbers")));
        assert!(!preflight(&[], Half::Left).ok());
    }

    #[test]
    fn conflicting_markers_are_refused() {
        let p = preflight(&image("FOXGLOVE_HALF=left FOXGLOVE_HALF=right"), Half::Left);
        assert!(p.reasons.iter().any(|r| r.contains("conflicting")));
    }
}
