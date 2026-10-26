//! Auto-update (#65): a check at startup and on demand, a signed manifest, a verified MSI
//! download, and a copied helper that runs the (per-user, no UAC) upgrade after the app exits.
use foxglove_update::{Manifest, Release, Result, Transport};
use std::{path::PathBuf, sync::Mutex, time::Duration};
#[derive(Clone, Default)]
struct State {
    busy: bool,
    message: String,
    available: Option<(Manifest, String)>,
}
static STATE: Mutex<State> = Mutex::new(State {
    busy: false,
    message: String::new(),
    available: None,
});
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}
/// Repaints the settings window when a check or download starts or ends (#282): the worker
/// says when, instead of the UI redrawing every 200 ms while busy. Set by `group`.
static WAKE: std::sync::OnceLock<crate::ui_wake::UiWake> = std::sync::OnceLock::new();
fn wake() {
    if let Some(w) = WAKE.get() {
        w.request_repaint();
    }
}
fn key() -> Result<ed_key::Key> {
    ed_key::parse(include_str!("../../resources/update-key.pub"))
}
// Keep the cryptographic key type inferred outside this adapter.
mod ed_key {
    pub type Key = foxglove_update::PublicKey;
    pub fn parse(s: &str) -> foxglove_update::Result<Key> {
        foxglove_update::public_key(s)
    }
}
struct Http;
impl Transport for Http {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        if !url.starts_with("https://") {
            return Err("Update URL must use HTTPS".into());
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .https_only(true)
            .build()
            .into();
        Ok(agent
            .get(url)
            .header("User-Agent", "Foxglove-updater")
            .call()?
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()?)
    }
}
/// The `[updates]` section of the settings store.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct UpdateSettings {
    check_at_startup: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings {
            check_at_startup: true,
        }
    }
}

fn startup_enabled() -> bool {
    super::store::load_or_migrate("updates", || {
        // The old updates.ini, from before the store.
        let ini = crate::settings::Settings::config_file_path()?.with_file_name("updates.ini");
        let on = ini::Ini::load_from_file(&ini)
            .ok()?
            .section(Some("updates"))?
            .get("check_at_startup")?
            .parse()
            .ok()?;
        Some((
            UpdateSettings {
                check_at_startup: on,
            },
            vec![ini],
        ))
    })
    .check_at_startup
}
fn folder() -> Result<PathBuf> {
    let p = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?)
        .join("Foxglove")
        .join("updates");
    std::fs::create_dir_all(&p)?;
    Ok(p)
}
pub fn start() {
    if let Ok(p) = folder() {
        if let Ok(error) = std::fs::read_to_string(p.join("error.txt")) {
            state().message = error;
            let _ = std::fs::remove_file(p.join("error.txt"));
        }
    }
    if key().is_ok() && startup_enabled() {
        let _ = std::thread::Builder::new()
            .name("update-startup".into())
            .spawn(|| {
                std::thread::sleep(Duration::from_secs(4));
                check();
            });
    }
}
fn check() {
    if key().is_err() {
        state().message = "Automatic updates are off in this version".into();
        return;
    }
    {
        let mut s = state();
        if s.busy {
            return;
        }
        s.busy = true;
        s.message = "Checking for updates…".into();
    }
    wake();
    worker(|| {
        let mut releases = Vec::new();
        for page in 1..=100 {
            let batch: Vec<Release> = serde_json::from_slice(&Http.get(&format!("https://api.github.com/repos/BanditF/foxglove/releases?per_page=100&page={page}"), 8*1024*1024)?)?;
            let done = batch.len() < 100;
            releases.extend(batch);
            if done {
                break;
            }
            if page == 100 {
                return Err("Too many release pages".into());
            }
        }
        let available =
            foxglove_update::check(&Http, &releases, env!("CARGO_PKG_VERSION"), &key()?)?;
        let mut s = state();
        s.message = if available.is_some() {
            "Update available"
        } else {
            "You are up to date"
        }
        .into();
        s.available = available;
        Ok(())
    });
}
fn worker(work: impl FnOnce() -> Result<()> + Send + 'static) {
    if let Err(e) = std::thread::Builder::new()
        .name("foxglove-update".into())
        .spawn(move || {
            let result = work();
            {
                let mut s = state();
                s.busy = false;
                if let Err(e) = result {
                    s.message = e.to_string();
                }
            }
            wake();
        })
    {
        let mut s = state();
        s.busy = false;
        s.message = e.to_string();
    }
}
pub fn available_version() -> Option<String> {
    state().available.as_ref().map(|(m, _)| m.version.clone())
}
/// Where releases are published, for builds that can't update themselves.
const RELEASES_PAGE: &str = "https://github.com/BanditF/foxglove/releases";

pub fn group(ui: &mut egui::Ui) {
    let _ = WAKE.set(crate::ui_wake::UiWake::from_ctx(ui.ctx()));
    // The window theme and the version in one card (#390).
    super::settings_ui::section(ui, "Theme and updates", |ui| {
        super::theme::picker(ui);
        super::autostart::checkbox(ui);
        let version = format!("Foxglove {}", env!("CARGO_PKG_VERSION"));
        // Builds without the release signing key (0.1) can't verify an update, so they don't
        // offer one: they point at the release page instead.
        if key().is_err() {
            ui.horizontal(|ui| {
                ui.label(version);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.hyperlink_to("Releases on GitHub", RELEASES_PAGE);
                });
            });
            ui.weak(
                "Automatic updates are off in this version. New versions are published on GitHub.",
            );
            return;
        }
        let s = state().clone();
        ui.horizontal(|ui| {
            ui.label(version);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!s.busy, egui::Button::new("Check now"))
                    .clicked()
                {
                    check();
                }
            });
        });
        if !s.message.is_empty() {
            ui.weak(&s.message);
        }
        let mut enabled = startup_enabled();
        if ui
            .checkbox(&mut enabled, "Check for updates at startup")
            .changed()
        {
            let result = super::store::save(
                "updates",
                &UpdateSettings {
                    check_at_startup: enabled,
                },
            );
            if let Err(e) = result {
                state().message = e.to_string();
            }
        }
        if let Some((m, url)) = s.available {
            ui.label(&m.notes);
            ui.hyperlink_to("View release", url);
            #[cfg(windows)]
            if ui
                .add_enabled(
                    !s.busy,
                    egui::Button::new(format!("Install v{}", m.version)),
                )
                .clicked()
            {
                state().busy = true;
                worker(move || {
                    state().message = "Downloading installer…".into();
                    let p = folder()?.join("foxglove-x64.msi");
                    let bytes = Http.get(&m.msi.url, m.msi.size)?;
                    std::fs::write(&p, bytes)?;
                    foxglove_update::verify_file(&p, &m.msi)?;
                    let exe = std::env::current_exe()?;
                    let helper = folder()?.join("foxglove-update-helper.exe");
                    std::fs::copy(&exe, &helper)?;
                    std::process::Command::new(helper)
                        .arg("--apply-update")
                        .arg(p)
                        .arg("--pid")
                        .arg(std::process::id().to_string())
                        .arg("--restart")
                        .arg(exe)
                        .spawn()?;
                    std::process::exit(0);
                });
            }
        }
    });
}
#[cfg(not(windows))]
pub fn helper_entry() -> bool {
    false
}
#[cfg(windows)]
pub fn helper_entry() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|a| a != "--apply-update") {
        return false;
    }
    let run = || -> Result<()> {
        use windows::core::{w, PCWSTR};
        use windows::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
            },
            UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
        };
        if args.len() != 7 || args[3] != "--pid" || args[5] != "--restart" {
            return Err("Invalid update helper arguments".into());
        }
        let pid: u32 = args[4].to_str().ok_or("Invalid PID")?.parse()?;
        unsafe {
            if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                let wait = WaitForSingleObject(process, 60000);
                let _ = CloseHandle(process);
                if wait != WAIT_OBJECT_0 {
                    return Err("Timed out waiting for Foxglove to exit".into());
                }
            }
            let msi = PathBuf::from(&args[2]);
            let path = msi.to_str().ok_or("Invalid installer path")?;
            if path.contains('"') {
                return Err("Invalid installer path".into());
            }
            let params: Vec<u16> = format!("/i \"{path}\" /passive /norestart")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS,
                // Foxglove (#444): the MSI is per user, so no UAC. (A per-machine MSI would
                // still ask for it itself.)
                lpVerb: w!("open"),
                lpFile: w!("msiexec.exe"),
                lpParameters: PCWSTR(params.as_ptr()),
                nShow: 1,
                ..Default::default()
            };
            ShellExecuteExW(&mut info)?;
            WaitForSingleObject(info.hProcess, u32::MAX);
            let mut code = 1;
            let result = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            result?;
            if code != 0 && code != 3010 {
                return Err(format!("Windows Installer failed ({code})").into());
            }
        }
        Ok(())
    };
    if let Err(e) = run() {
        if let Ok(p) = folder() {
            let _ = std::fs::write(p.join("error.txt"), format!("Update failed: {e}"));
        }
    }
    if let Some(exe) = args.get(6) {
        let _ = std::process::Command::new(exe).spawn();
    }
    true
}
