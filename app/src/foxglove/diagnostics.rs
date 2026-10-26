//! Foxglove (#371): "Copy diagnostics" in the status strip. One click puts a plain-text report
//! on the clipboard for a bug report: app and firmware versions, the connection, HELLO
//! capabilities, plugins and who holds the lights, OS and GPU basics, and the last log lines.
//!
//! Nothing personal or secret goes in: the user's name, the PC's name and the profile path are
//! replaced in every line, log lines are masked again for secrets (`app_log::redact`), and
//! plugin settings, tokens and credentials are never read.

use std::sync::Mutex;

/// Log lines in the report.
const LOG_LINES: usize = 200;

/// The GPU as the overlay's GL context names it, set once the window is up.
static GPU: Mutex<Option<String>> = Mutex::new(None);

/// From the window's creation: the GL renderer and version strings.
pub fn set_gpu(renderer: &str, version: &str) {
    *GPU.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{renderer} (OpenGL {version})"));
}

/// A plugin as the report lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plugin {
    pub id: String,
    pub version: String,
    pub enabled: bool,
}

/// Everything the report says, gathered first so the text is a pure function of it.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub app_version: String,
    pub app_build: String,
    pub os: String,
    pub gpu: Option<String>,
    pub keyboard: Option<String>,
    pub board: Option<String>,
    /// USB or Bluetooth (with the profile), or unknown.
    pub connection: String,
    pub left_firmware: String,
    pub right_firmware: String,
    pub right_battery: Option<i64>,
    /// The left's own level (firmware with LEFT_BATTERY).
    pub left_battery: Option<i64>,
    pub hello: Option<digitalis::protocol::Hello>,
    pub plugins: Vec<Plugin>,
    /// Who holds each device's lights: `device: plugin (priority)`.
    pub lights: Vec<String>,
    pub log: Vec<String>,
}

/// Values replaced in every line: the profile path before the bare name, longest first.
pub struct Personal(Vec<(String, &'static str)>);

impl Personal {
    pub fn from_env() -> Personal {
        let mut v = Vec::new();
        for (var, placeholder) in [
            ("USERPROFILE", "%USERPROFILE%"),
            ("HOME", "~"),
            ("COMPUTERNAME", "<pc>"),
            ("USERNAME", "<user>"),
            ("USER", "<user>"),
        ] {
            if let Some(value) = std::env::var(var).ok().filter(|v| v.len() >= 2) {
                v.push((value, placeholder));
            }
        }
        Personal::new(v)
    }

    fn new(mut v: Vec<(String, &'static str)>) -> Personal {
        v.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        Personal(v)
    }

    /// `text` with each personal value (any case) replaced.
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (value, placeholder) in &self.0 {
            out = replace_ignore_case(&out, value, placeholder);
        }
        out
    }
}

fn replace_ignore_case(text: &str, needle: &str, with: &str) -> String {
    let (lower, needle) = (text.to_lowercase(), needle.to_lowercase());
    // Lowercasing can change byte lengths outside ASCII; fall back to an exact match then.
    if lower.len() != text.len() {
        return text.replace(&needle, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in lower.match_indices(&needle) {
        if at < last || !text.is_char_boundary(at) {
            continue;
        }
        out.push_str(&text[last..at]);
        out.push_str(with);
        last = at + needle.len();
    }
    out.push_str(&text[last..]);
    out
}

/// The names of the HELLO capability bits that are set, then any unnamed ones.
fn capability_names(bits: u32) -> String {
    use digitalis::protocol::capabilities as c;
    let named = [
        (c::PANEL, "PANEL"),
        (c::ART_RELAY, "ART_RELAY"),
        (c::SETTINGS_RGB, "SETTINGS_RGB"),
        (c::ART_STATUS_EVENT, "ART_STATUS_EVENT"),
        (c::CONN_STATE, "CONN_STATE"),
        (c::POSITION, "POSITION"),
        (c::SCREEN, "SCREEN"),
        (c::INVOKE_REFUSES, "INVOKE_REFUSES"),
        (c::CONN_STATE_EVENT, "CONN_STATE_EVENT"),
        (c::BOOTLOADER, "BOOTLOADER"),
        (c::ART_SLOTS, "ART_SLOTS"),
        (c::RGB_STATE_EVENT, "RGB_STATE_EVENT"),
        (c::BUILD_INFO, "BUILD_INFO"),
        (c::SCREENS, "SCREENS"),
        (c::PANEL_HOLD, "PANEL_HOLD"),
    ];
    let mut names: Vec<String> = named
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| (*n).to_string())
        .collect();
    let rest = named.iter().fold(bits, |acc, (b, _)| acc & !b);
    if rest != 0 {
        names.push(format!("other 0x{rest:x}"));
    }
    if names.is_empty() {
        "none".into()
    } else {
        names.join(" ")
    }
}

/// The report's text.
pub fn report(f: &Facts, personal: &Personal) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let mut line = |k: &str, v: &str| {
        let _ = writeln!(s, "{k:<16}{v}");
    };
    line("Foxglove", &format!("{} ({})", f.app_version, f.app_build));
    line("OS", &f.os);
    line("GPU", f.gpu.as_deref().unwrap_or("not known yet"));
    line(
        "Keyboard",
        f.keyboard.as_deref().unwrap_or("none connected"),
    );
    line("Board profile", f.board.as_deref().unwrap_or("none"));
    line("Connection", &f.connection);
    line("Left firmware", &f.left_firmware);
    line("Right firmware", &f.right_firmware);
    let level = |b: Option<i64>| b.map_or("not reported".into(), |b| format!("{b}%"));
    line("Left battery", &level(f.left_battery));
    line("Right battery", &level(f.right_battery));
    match &f.hello {
        Some(h) => {
            line(
                "HELLO",
                &format!(
                    "v{} · panel {}×{} · right link {} · panel timeout {} s",
                    h.version, h.width, h.height, h.right_link, h.panel_timeout_s
                ),
            );
            line(
                "Capabilities",
                &format!(
                    "0x{:08x} {}",
                    h.capabilities,
                    capability_names(h.capabilities)
                ),
            );
        }
        None => line("HELLO", "no reply"),
    }
    if f.plugins.is_empty() {
        line("Plugins", "none");
    }
    for (i, p) in f.plugins.iter().enumerate() {
        line(
            if i == 0 { "Plugins" } else { "" },
            &format!(
                "{} {}{}",
                p.id,
                p.version,
                if p.enabled { "" } else { " (off)" }
            ),
        );
    }
    line(
        "Lights held by",
        &if f.lights.is_empty() {
            "nobody (the keyboard's own)".into()
        } else {
            f.lights.join("; ")
        },
    );
    let _ = writeln!(s, "\nLast {} log lines:", f.log.len());
    for l in &f.log {
        let _ = writeln!(s, "{}", super::app_log::redact(l));
    }
    personal.scrub(&s)
}

/// The OS name and build.
fn os() -> String {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        let key = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        if let Ok(k) = key {
            let get = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
            let ubr = k
                .get_value::<u32, _>("UBR")
                .map(|u| format!(".{u}"))
                .unwrap_or_default();
            // ProductName still says "Windows 10" on Windows 11; the build tells them apart.
            let build: u32 = get("CurrentBuild").parse().unwrap_or(0);
            let name = if build >= 22000 {
                get("ProductName").replace("Windows 10", "Windows 11")
            } else {
                get("ProductName")
            };
            return format!("{name} {} (build {build}{ubr})", get("DisplayVersion"));
        }
    }
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Gathers the facts from the running app.
pub fn collect(ctx: &egui::Context) -> Facts {
    use foxglove_devices::facts::Value;
    let facts = super::devices::bus().facts();
    let level = |part: &str| match facts.get(&format!("keyboard.{part}.battery")) {
        Some(Value::Int(b)) => Some(b),
        _ => None,
    };
    let (battery, left_battery) = (level("right"), level("left"));
    let link = match facts.get("keyboard.link") {
        Some(Value::Text(t)) => t,
        _ => String::new(),
    };
    let summary = super::connection::summary(ctx);
    let connected = facts.get("keyboard.connected") == Some(Value::Bool(true));
    let connection = connection_line(
        summary.usb,
        &link,
        summary.profile,
        super::devices::host_link().filter(|_| connected),
    );
    let (left_firmware, right_firmware) = match super::keyboard_settings::firmware(ctx) {
        super::keyboard_settings::Firmware::Known { left, right } => {
            let text = |r: Result<digitalis::protocol::BuildInfo, String>| match r {
                Ok(b) => format!("{} · {}", b.version, b.sha),
                Err(e) => format!("unknown ({e})"),
            };
            (text(left), text(right))
        }
        other => {
            let t = format!("{other:?}");
            (t.clone(), t)
        }
    };
    let snapshot = super::screens::snapshot();
    let plugins = super::plugin_install::installed()
        .into_iter()
        .map(|p| Plugin {
            id: p.manifest.id,
            version: p.manifest.version,
            enabled: p.record.enabled,
        })
        .collect();
    let lights = super::lighting_leases::holders()
        .into_iter()
        .map(|(device, conn, priority)| {
            let who = foxglove_plugin_host::plugin_of(conn)
                .unwrap_or_else(|| format!("a tool (connection {conn})"));
            format!("{device}: {who} ({priority:?})")
        })
        .collect();
    Facts {
        app_version: env!("CARGO_PKG_VERSION").into(),
        app_build: format!(
            "{}, {}",
            option_env!("FOXGLOVE_GIT_SHA").unwrap_or("build unknown"),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        ),
        os: os(),
        gpu: GPU.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        keyboard: (!snapshot.product.is_empty()).then(|| snapshot.product.clone()),
        board: super::boards::current().map(|b| format!("{} ({})", b.name, b.id)),
        connection,
        left_firmware,
        right_firmware,
        right_battery: battery,
        left_battery,
        hello: snapshot.hello,
        plugins,
        lights,
        log: super::app_log::recent(LOG_LINES),
    }
}

/// The "Connection" line: how the keyboard types to this PC (USB, or Bluetooth with its profile)
/// and how Foxglove's host link reaches it, which can differ (USB Raw HID is silent before
/// firmware #434, so the link runs over BLE): `USB (typing) · host link BLE`.
fn connection_line(
    usb: Option<bool>,
    link_fact: &str,
    profile: Option<u8>,
    host_link: Option<&str>,
) -> String {
    let typing = match (usb, link_fact) {
        (Some(true), _) | (None, "usb") => "USB".to_string(),
        (Some(false), _) | (None, "ble") => format!(
            "Bluetooth{}",
            profile
                .map(|p| format!(", profile {}", p + 1))
                .unwrap_or_default()
        ),
        _ => "unknown".into(),
    };
    match host_link {
        Some(link) => format!("{typing} (typing) · host link {link}"),
        None => typing,
    }
}

/// Collects, writes and copies the report to the clipboard.
pub fn copy(ctx: &egui::Context) {
    let text = report(&collect(ctx), &Personal::from_env());
    ctx.copy_text(text);
}

/// The personal values the sample's made-up user has.
#[cfg(test)]
pub(crate) fn sample_personal() -> Personal {
    Personal::new(vec![
        ("C:\\Users\\Bandit".into(), "%USERPROFILE%"),
        ("Bandit".into(), "<user>"),
        ("BANDIT-PC".into(), "<pc>"),
    ])
}

#[cfg(test)]
pub(crate) fn sample() -> Facts {
    Facts {
        app_version: "0.2.0".into(),
        app_build: "c7560d7, release".into(),
        os: "Windows 11 Pro 25H2 (build 26300.1)".into(),
        gpu: Some("NVIDIA GeForce RTX/PCIe/SSE2 (OpenGL 4.6.0 NVIDIA 581.57)".into()),
        keyboard: Some("Eyelash Corne".into()),
        board: Some("Eyelash Corne (eyelash_corne)".into()),
        connection: "USB (typing) · host link BLE".into(),
        left_firmware: "0.1.0 dev · 2ae9522".into(),
        right_firmware: "0.1.0 dev · 2ae9522".into(),
        right_battery: Some(61),
        left_battery: Some(95),
        hello: Some(digitalis::protocol::Hello {
            version: 3,
            width: 64,
            height: 52,
            right_link: 2,
            capabilities: digitalis::protocol::capabilities::PANEL | digitalis::protocol::capabilities::BUILD_INFO | (1 << 30),
            ..Default::default()
        }),
        plugins: vec![
            Plugin { id: "pomodoro".into(), version: "1.2.0".into(), enabled: true },
            Plugin { id: "obs-status".into(), version: "0.3.1".into(), enabled: false },
        ],
        lights: vec!["zmk:1d50:615e: pomodoro (Normal)".into()],
        log: vec![
            "2026-10-25 14:10:07Z Foxglove: loaded C:\\Users\\Bandit\\AppData\\Roaming\\Foxglove\\foxglove.toml".into(),
            "2026-10-25 14:10:08Z Foxglove obs: token=hunter2 accepted on BANDIT-PC".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_connection_names_typing_and_the_host_link() {
        assert_eq!(
            connection_line(Some(true), "usb", Some(0), Some("BLE")),
            "USB (typing) · host link BLE"
        );
        assert_eq!(
            connection_line(Some(false), "ble", Some(1), Some("BLE")),
            "Bluetooth, profile 2 (typing) · host link BLE"
        );
        assert_eq!(
            connection_line(None, "usb", None, None),
            "USB",
            "no host link"
        );
        assert_eq!(connection_line(None, "", None, None), "unknown");
    }

    fn personal() -> Personal {
        Personal::new(vec![
            ("C:\\Users\\Bandit".into(), "%USERPROFILE%"),
            ("Bandit".into(), "<user>"),
            ("BANDIT-PC".into(), "<pc>"),
        ])
    }

    #[test]
    fn the_report_has_the_facts() {
        let text = report(&sample(), &personal());
        for want in [
            "Foxglove        0.2.0 (c7560d7, release)",
            "Windows 11",
            "NVIDIA GeForce",
            "Left firmware   0.1.0 dev · 2ae9522",
            "Connection      USB",
            "Left battery    95%",
            "Right battery   61%",
            "PANEL BUILD_INFO other 0x40000000",
            "pomodoro 1.2.0",
            "obs-status 0.3.1 (off)",
            "Lights held by  zmk:1d50:615e: pomodoro (Normal)",
            "Last 2 log lines:",
        ] {
            assert!(text.contains(want), "missing {want:?} in\n{text}");
        }
    }

    #[test]
    fn secrets_and_personal_data_stay_out() {
        let text = report(&sample(), &personal());
        for gone in ["hunter2", "Bandit", "BANDIT-PC", "bandit-pc"] {
            assert!(!text.contains(gone), "{gone:?} leaked in\n{text}");
        }
        assert!(text.contains("%USERPROFILE%\\AppData\\Roaming\\Foxglove\\foxglove.toml"));
        assert!(text.contains("token=*** accepted on <pc>"));
    }

    #[test]
    fn scrubbing_ignores_case_and_keeps_the_rest() {
        let p = personal();
        assert_eq!(
            p.scrub("c:\\users\\bandit\\x and BANDIT"),
            "%USERPROFILE%\\x and <user>"
        );
        assert_eq!(p.scrub("nothing here · ünï"), "nothing here · ünï");
        assert_eq!(replace_ignore_case("aXa", "x", "-"), "a-a");
    }

    #[test]
    fn no_keyboard_reads_plainly() {
        let text = report(
            &Facts {
                connection: "unknown".into(),
                ..Default::default()
            },
            &personal(),
        );
        assert!(text.contains("Keyboard        none connected"));
        assert!(text.contains("HELLO           no reply"));
        assert!(text.contains("Plugins         none"));
        assert!(text.contains("nobody (the keyboard's own)"));
    }

    #[test]
    fn capability_bits_are_named() {
        use digitalis::protocol::capabilities as c;
        assert_eq!(capability_names(0), "none");
        assert_eq!(
            capability_names(c::PANEL | c::PANEL_HOLD),
            "PANEL PANEL_HOLD"
        );
    }
}
