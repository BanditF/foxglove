//! Host-side checks that don't go through the Foxglove link: how each half shows up on USB,
//! and how much CPU the app burns while idle. They read Windows' device list and process
//! times through PowerShell; on other systems they fail with a clear message.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

/// This PC's halves: the defaults when neither the environment nor `tools/local.toml` names
/// others (`scripts/local_config.py` has the same keys and order).
pub const LEFT_SERIAL: &str = "1E89EF085AA8A635";
pub const RIGHT_SERIAL: &str = "35B4541F78DE9CF8";

/// The two halves' USB serials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Serials {
    pub left: String,
    pub right: String,
}

impl Default for Serials {
    fn default() -> Self {
        Self {
            left: LEFT_SERIAL.into(),
            right: RIGHT_SERIAL.into(),
        }
    }
}

/// The serials from `FOXGLOVE_LEFT_SERIAL` / `FOXGLOVE_RIGHT_SERIAL`, else `[keyboard]
/// left_serial` / `right_serial` in `toml` (a tools/local.toml), else this PC's. A value that
/// isn't plain letters and digits is ignored, as it goes into a PowerShell pattern.
pub fn serials_from(env: impl Fn(&str) -> Option<String>, toml: Option<&str>) -> Serials {
    let table: Option<toml::Table> = toml.and_then(|t| t.parse().ok());
    let file = |key: &str| {
        table
            .as_ref()?
            .get("keyboard")?
            .get(key)?
            .as_str()
            .map(str::to_string)
    };
    let ok = |v: &String| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric());
    let pick = |var: &str, key: &str, default: &str| {
        env(var)
            .filter(ok)
            .or_else(|| file(key).filter(ok))
            .unwrap_or_else(|| default.to_string())
    };
    Serials {
        left: pick("FOXGLOVE_LEFT_SERIAL", "left_serial", LEFT_SERIAL),
        right: pick("FOXGLOVE_RIGHT_SERIAL", "right_serial", RIGHT_SERIAL),
    }
}

/// Where tools/local.toml may be: `FOXGLOVE_LOCAL_TOML`, this checkout's `tools\`, then the
/// workspace's (the folder holding the main checkout), as in `scripts/local_config.py`.
fn local_toml() -> Option<String> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("FOXGLOVE_LOCAL_TOML").filter(|v| !v.is_empty()) {
        paths.push(p.into());
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    if let Some(top) = git(&["rev-parse", "--show-toplevel"]) {
        paths.push(std::path::Path::new(&top).join("tools/local.toml"));
    }
    if let Some(common) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        if let Some(ws) = std::path::Path::new(&common)
            .parent()
            .and_then(|p| p.parent())
        {
            paths.push(ws.join("tools/local.toml"));
        }
    }
    paths
        .into_iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
}

/// This PC's serials (read once).
pub fn serials() -> &'static Serials {
    static S: std::sync::OnceLock<Serials> = std::sync::OnceLock::new();
    S.get_or_init(|| serials_from(|v| std::env::var(v).ok(), local_toml().as_deref()))
}
const RUNNING: &str = "USB\\VID_1D50&PID_615E\\";
const BOOTLOADER: &str = "USB\\VID_239A&PID_00B3\\";

/// One present USB device belonging to a half: running firmware or the UF2 bootloader.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct UsbEntry {
    #[serde(rename = "InstanceId")]
    pub instance_id: String,
    #[serde(rename = "Status")]
    pub status: String,
    /// Interfaces Windows created under it (HID, Raw HID, Studio serial on the left).
    #[serde(rename = "Children")]
    pub children: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Info,
    Warn,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HalfUsb {
    pub half: &'static str,
    pub verdict: Verdict,
    pub message: String,
}

/// Sorts out what each half is doing on USB. The right half's peripheral firmware brings up
/// USB with no interfaces, so Windows lists it with problem 28 and no children: expected. A
/// right half WITH interfaces is running left or settings_reset firmware.
pub fn classify(entries: &[UsbEntry]) -> Vec<HalfUsb> {
    classify_with(entries, serials())
}

/// [`classify`] for the given halves.
pub fn classify_with(entries: &[UsbEntry], serials: &Serials) -> Vec<HalfUsb> {
    let find = |prefix: &str, serial: &str| {
        entries.iter().find(|e| {
            e.instance_id
                .eq_ignore_ascii_case(&format!("{prefix}{serial}"))
        })
    };
    let half = |half: &'static str, verdict, message: String| HalfUsb {
        half,
        verdict,
        message,
    };
    let mut out = Vec::new();
    for (name, serial) in [("left", &serials.left), ("right", &serials.right)] {
        if find(BOOTLOADER, serial).is_some() {
            out.push(half(
                name,
                Verdict::Warn,
                format!("{name} half is in the UF2 bootloader"),
            ));
            continue;
        }
        let running = find(RUNNING, serial);
        out.push(match (name, running) {
            ("left", Some(e)) if e.status == "OK" && e.children > 0 => half(
                name,
                Verdict::Ok,
                format!("running, {} USB interfaces", e.children),
            ),
            ("left", Some(e)) => half(
                name,
                Verdict::Fail,
                format!(
                    "left enumerates badly: status {}, {} interfaces",
                    e.status, e.children
                ),
            ),
            ("left", None) => half(
                name,
                Verdict::Warn,
                "left is not on USB (Bluetooth only?)".into(),
            ),
            (_, None) => half(name, Verdict::Info, "right is not on USB (on battery)".into()),
            (_, Some(e)) if e.children == 0 => half(
                name,
                Verdict::Info,
                format!(
                    "right on USB power with peripheral firmware: no interfaces, status {} (expected)",
                    e.status
                ),
            ),
            (_, Some(e)) => half(
                name,
                Verdict::Fail,
                format!(
                    "right exposes {} USB interfaces: left or settings_reset firmware on it?",
                    e.children
                ),
            ),
        });
    }
    out
}

fn powershell(script: &str) -> Result<String> {
    ensure!(cfg!(windows), "host checks need Windows");
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("could not start PowerShell")?;
    ensure!(
        out.status.success(),
        "PowerShell failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The halves' present USB devices, with their interface counts.
pub fn usb_entries() -> Result<Vec<UsbEntry>> {
    let Serials {
        left: left_serial,
        right: right_serial,
    } = serials();
    let script = format!(
        "$d = @(Get-PnpDevice -PresentOnly | Where-Object {{ $_.InstanceId -match \
         '^USB\\\\VID_(1D50&PID_615E|239A&PID_00B3)\\\\({left_serial}|{right_serial})$' }} | \
         ForEach-Object {{ $c = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName \
         DEVPKEY_Device_Children -ErrorAction SilentlyContinue).Data; [pscustomobject]@{{ \
         InstanceId = $_.InstanceId; Status = [string]$_.Status; Children = @($c | \
         Where-Object {{ $_ }}).Count }} }}); ConvertTo-Json -Compress -InputObject $d"
    );
    let json = powershell(&script)?;
    Ok(serde_json::from_str(if json.is_empty() {
        "[]"
    } else {
        &json
    })?)
}

/// The running Foxglove, if any: its exe path and file version (for the report).
pub fn app_info() -> crate::report::App {
    let Ok(out) = powershell(
        "$p = Get-Process | Where-Object { $_.ProcessName -like 'foxglove*' -and $_.ProcessName -ne 'foxglove-lab' } | Select-Object -First 1;          if ($p) { ConvertTo-Json -Compress @{ path = $p.Path; version =          [string]$p.MainModule.FileVersionInfo.ProductVersion } }",
    ) else {
        return crate::report::App::default();
    };
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
    let text = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
    crate::report::App {
        running: v.is_object(),
        path: text("path"),
        version: text("version"),
    }
}

/// Total CPU time (ms) used so far by every process with this name; errors if none runs.
pub fn process_cpu_ms(name: &str) -> Result<f64> {
    ensure!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "bad process name {name:?}"
    );
    let ms = powershell(&format!(
        "(Get-Process -Name {name} -ErrorAction Stop | ForEach-Object {{ \
         $_.TotalProcessorTime.TotalMilliseconds }} | Measure-Object -Sum).Sum"
    ))
    .with_context(|| format!("{name} is not running"))?;
    Ok(ms.parse()?)
}

/// Seconds since the youngest process with this name started; errors if none runs.
pub fn process_age_s(name: &str) -> Result<f64> {
    ensure!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "bad process name {name:?}"
    );
    let s = powershell(&format!(
        "$p = Get-Process -Name {name} -ErrorAction Stop | Sort-Object StartTime -Descending | \
         Select-Object -First 1; [string][math]::Round(((Get-Date) - $p.StartTime).TotalSeconds, 1)"
    ))
    .with_context(|| format!("{name} is not running"))?;
    Ok(s.trim().replace(',', ".").parse()?)
}

/// The app's settings store (foxglove.toml), where the app finds it: in FOXGLOVE_CONFIG_DIR
/// when set, else `%APPDATA%\Foxglove` (the folder above its `config\settings.ini`).
pub fn store_path() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("FOXGLOVE_CONFIG_DIR") {
        return Some(std::path::PathBuf::from(dir).join("foxglove.toml"));
    }
    std::env::var_os("APPDATA").map(|d| {
        std::path::PathBuf::from(d)
            .join("Foxglove")
            .join("foxglove.toml")
    })
}

/// What the left screen shows, from the store's text, as the app reads it
/// (screens_settings.rs, store.rs): `[screens]` with the active profile's overrides on top;
/// drive_left false is the keyboard's own screen, else the layout (info when unset).
pub fn left_layout(text: &str) -> String {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return "unknown (foxglove.toml isn't valid TOML)".into();
    };
    let get = |path: &[&str]| -> Option<&toml::Value> {
        let mut v = doc.get(path[0])?;
        for p in &path[1..] {
            v = v.as_table()?.get(*p)?;
        }
        Some(v)
    };
    let profile = get(&["profiles", "active"])
        .and_then(|v| v.as_str())
        .filter(|n| get(&["profiles", "list", n]).is_some());
    let key = |k: &str| {
        profile
            .and_then(|n| get(&["profiles", "list", n, "screens", k]))
            .or_else(|| get(&["screens", k]))
    };
    if key("drive_left").and_then(|v| v.as_bool()) == Some(false) {
        return "the keyboard's own screen (drive_left = false: Foxglove draws nothing)".into();
    }
    let layout = key("layout").and_then(|v| v.as_str()).unwrap_or("info");
    match profile {
        Some(n) => format!("{layout} (profile {n})"),
        None => layout.into(),
    }
}

/// The left screen's layout as saved now, with where it was read, for the report.
pub fn left_layout_now() -> String {
    match store_path() {
        Some(p) => match std::fs::read_to_string(&p) {
            Ok(text) => format!("{} (from {})", left_layout(&text), p.display()),
            Err(_) => format!("info (no {}: the defaults)", p.display()),
        },
        None => "unknown (no settings folder)".into(),
    }
}

/// Replaces `%NAME%` with environment variables (unknown names stay as written).
pub fn expand_env(text: &str) -> String {
    let parts: Vec<&str> = text.split('%').collect();
    let mut out = String::from(parts[0]);
    let mut i = 1;
    while i < parts.len() {
        match (std::env::var(parts[i]), i + 1 < parts.len()) {
            (Ok(v), true) if !parts[i].is_empty() => {
                out.push_str(&v);
                out.push_str(parts[i + 1]);
                i += 2;
            }
            _ => {
                out.push('%');
                out.push_str(parts[i]);
                i += 1;
            }
        }
    }
    out
}

/// Sets `[screens] key = value` in Foxglove's settings store (foxglove.toml), typed as the app
/// reads it: drive_left a bool, fps an integer, layout a string. Other tables stay as they were.
pub fn set_store_screens(text: &str, key: &str, value: &str) -> Result<String> {
    let mut doc: toml::Table = if text.trim().is_empty() {
        toml::Table::new()
    } else {
        text.parse().context("foxglove.toml isn't valid TOML")?
    };
    doc.entry("schema").or_insert(toml::Value::Integer(1));
    let typed = match key {
        "drive_left" => toml::Value::Boolean(value.parse().context("drive_left is true or false")?),
        "fps" => toml::Value::Integer(value.parse().context("fps is a number")?),
        "layout" => {
            ensure!(
                value == "info" || value == "wallpaper",
                "layout is info or wallpaper"
            );
            toml::Value::String(value.into())
        }
        other => anyhow::bail!("unsupported [screens] key {other}"),
    };
    let screens = doc
        .entry("screens")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(screens) = screens else {
        anyhow::bail!("[screens] in foxglove.toml isn't a table");
    };
    screens.insert(key.into(), typed);
    Ok(toml::to_string(&doc)?)
}

/// Sets `key=value` in `[section]` of an INI text, adding the section or key when missing and
/// keeping everything else as it was.
pub fn set_ini(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut out = Vec::new();
    let (mut in_section, mut done, mut seen) = (false, false, false);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section && !done {
                out.push(format!("{key}={value}"));
                done = true;
            }
            in_section = trimmed.trim_matches(['[', ']']) == section;
            seen |= in_section;
        } else if in_section && trimmed.split('=').next().map(str::trim) == Some(key) {
            out.push(format!("{key}={value}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if !seen {
            out.push(format!("[{section}]"));
        }
        out.push(format!("{key}={value}"));
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(prefix: &str, serial: &str, status: &str, children: u32) -> UsbEntry {
        UsbEntry {
            instance_id: format!("{prefix}{serial}"),
            status: status.into(),
            children,
        }
    }
    fn verdicts(entries: &[UsbEntry]) -> Vec<Verdict> {
        classify_with(entries, &Serials::default())
            .iter()
            .map(|h| h.verdict)
            .collect()
    }

    #[test]
    fn normal_setup_left_ok_right_bare_on_usb_power() {
        let e = [
            entry(RUNNING, LEFT_SERIAL, "OK", 3),
            entry(RUNNING, RIGHT_SERIAL, "Error", 0),
        ];
        assert_eq!(verdicts(&e), [Verdict::Ok, Verdict::Info]);
        assert_eq!(verdicts(&e[..1]), [Verdict::Ok, Verdict::Info]);
    }

    #[test]
    fn right_with_interfaces_is_wrong_firmware() {
        let e = [
            entry(RUNNING, LEFT_SERIAL, "OK", 3),
            entry(RUNNING, RIGHT_SERIAL, "OK", 3),
        ];
        let r = classify_with(&e, &Serials::default());
        assert_eq!(r[1].verdict, Verdict::Fail);
        assert!(r[1].message.contains("settings_reset"));
    }

    #[test]
    fn bootloader_absent_and_broken_left() {
        assert_eq!(
            verdicts(&[entry(BOOTLOADER, LEFT_SERIAL, "OK", 0)]),
            [Verdict::Warn, Verdict::Info]
        );
        assert_eq!(verdicts(&[]), [Verdict::Warn, Verdict::Info]);
        assert_eq!(
            verdicts(&[entry(RUNNING, LEFT_SERIAL, "Error", 0)]),
            [Verdict::Fail, Verdict::Info]
        );
    }

    #[test]
    fn store_screens_are_typed_and_other_tables_kept() {
        let toml = "schema = 1\n\n[profiles.list.Game]\n\n[screens]\nfps = 4\n";
        let out: toml::Table = set_store_screens(toml, "layout", "wallpaper")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(out["screens"]["layout"].as_str(), Some("wallpaper"));
        assert_eq!(out["screens"]["fps"].as_integer(), Some(4));
        assert!(out["profiles"]["list"].get("Game").is_some());
        let out: toml::Table = set_store_screens("", "drive_left", "false")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(out["schema"].as_integer(), Some(1));
        assert_eq!(out["screens"]["drive_left"].as_bool(), Some(false));
        assert!(set_store_screens("", "layout", "sideways").is_err());
        assert!(set_store_screens("", "fps", "fast").is_err());
    }

    #[test]
    fn left_layout_reads_screens_and_the_active_profile() {
        assert_eq!(left_layout(""), "info");
        assert_eq!(left_layout("[screens]\nlayout = \"stats\"\n"), "stats");
        assert!(
            left_layout("[screens]\ndrive_left = false\nlayout = \"stats\"\n")
                .starts_with("the keyboard's own screen")
        );
        let profiled = "[screens]\nlayout = \"info\"\n[profiles]\nactive = \"Game\"\n\
                        [profiles.list.Game.screens]\nlayout = \"wallpaper\"\n";
        assert_eq!(left_layout(profiled), "wallpaper (profile Game)");
        // A profile that's gone doesn't count.
        let gone = "[screens]\nlayout = \"stats\"\n[profiles]\nactive = \"Work\"\n";
        assert_eq!(left_layout(gone), "stats");
        assert!(left_layout("not = [toml").starts_with("unknown"));
    }

    #[test]
    fn ini_set_replaces_adds_and_keeps_the_rest() {
        let ini = "[screens]\ndrive_left=true\nfps=4\n[other]\nfps=9\n";
        assert_eq!(
            set_ini(ini, "screens", "drive_left", "false"),
            "[screens]\ndrive_left=false\nfps=4\n[other]\nfps=9\n"
        );
        assert_eq!(
            set_ini(
                "[screens]\nfps=4\n[other]\n",
                "screens",
                "drive_left",
                "true"
            ),
            "[screens]\nfps=4\ndrive_left=true\n[other]\n"
        );
        assert_eq!(set_ini("", "screens", "fps", "2"), "[screens]\nfps=2\n");
    }

    #[test]
    fn expands_known_environment_variables() {
        std::env::set_var("FOXGLOVE_LAB_TEST_DIR", "C:/t");
        assert_eq!(expand_env("%FOXGLOVE_LAB_TEST_DIR%/p.pbm"), "C:/t/p.pbm");
        assert_eq!(expand_env("100% sure"), "100% sure");
        assert_eq!(
            expand_env("%NOT_SET_ANYWHERE_X%/a"),
            "%NOT_SET_ANYWHERE_X%/a"
        );
    }

    #[test]
    fn rejects_odd_process_names() {
        assert!(process_cpu_ms("foo; Remove-Item x").is_err());
    }

    #[test]
    fn serials_come_from_the_environment_then_local_toml_then_this_pc() {
        let none = |_: &str| None;
        assert_eq!(serials_from(none, None), Serials::default());
        let toml = "[keyboard]\nleft_serial = \"AAAA1111\"\n";
        let s = serials_from(none, Some(toml));
        assert_eq!(
            (s.left.as_str(), s.right.as_str()),
            ("AAAA1111", RIGHT_SERIAL)
        );
        let env = |v: &str| (v == "FOXGLOVE_LEFT_SERIAL").then(|| "BBBB2222".to_string());
        assert_eq!(serials_from(env, Some(toml)).left, "BBBB2222");
        // Not a plain serial (it would go into a PowerShell pattern): ignored.
        let bad = |_: &str| Some("x|.*".to_string());
        assert_eq!(serials_from(bad, Some("not toml [")), Serials::default());
    }
}
