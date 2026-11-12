// SPDX-License-Identifier: MIT
//! `hideout.exe --adopt <folder>` (#1117): registers an `adopted` module, an install the hub did
//! not make (Arete's `arete hub` today), and exits. It opens no window and starts nothing.
//!
//! `<folder>` holds the module's `module.toml` with `kind = "adopted"`, an absolute `run[0]` that
//! is this PC's real program, and `hub` it needs. Nothing here knows any module: which program
//! and which id is data in that file. The registry copies the manifest into the hub's own
//! record and reads nothing at the external path beyond its existence ([`register_adopted`]). A
//! running hub notices the new inventory (`inventory_watch`) and shows the module's tile and
//! page; registering again updates the version and keeps the grants.
//!
//! [`register_adopted`]: hideout_registry::Registry::register_adopted

use std::path::Path;

use hideout_contract::module::{Context, Module};
use hideout_registry::Registry;

/// Registers the adopted module in `folder` into the hub folder `home`. The answer is the line
/// to print: the module's id and version, or why it was refused.
pub fn run(home: &Path, folder: &Path, now: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(folder.join("module.toml"))
        .map_err(|e| format!("{}: {e}", folder.join("module.toml").display()))?;
    let id = Module::parse(&text, &Context::default())
        .map_err(|e| format!("module.toml: {e}"))?
        .id;
    let mut reg = Registry::open_from_env(home).map_err(|e| format!("registry not read: {e}"))?;
    let m = reg
        .register_adopted(folder, now)
        .map_err(|e| format!("not registered: {e}"))?;
    Ok(format!(
        "registered {id} {} (installed separately)",
        m.version
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-11-12T08:00:00Z";

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hideout-adopt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn folder(name: &str, kind: &str, exe: &Path) -> std::path::PathBuf {
        let dir = temp(name);
        let exe = exe.display().to_string().replace('\\', "/");
        let text = format!(
            "id = \"vulpes\"\nname = \"AI\"\nversion = \"0.1.0\"\nkind = \"{kind}\"\n\
             family = \"vulpes\"\nhub = \"1.1\"\nrun = ['{exe}', 'hub']\n\
             api = {{ vulpes = \"1.0\" }}\n[activation]\non = [\"open\", \"call\"]\n"
        );
        std::fs::write(dir.join("module.toml"), text).unwrap();
        dir
    }

    /// The verb registers any adopted module from its folder: the path is the file's, nothing
    /// is Arete-specific. Registering again is an update; a missing program, another kind or
    /// no manifest is refused with a sentence and records nothing.
    #[test]
    fn registers_an_adopted_module_by_its_folder_and_refuses_the_rest() {
        let home = temp("home");
        let install = temp("install");
        let exe = install.join("tool.exe");
        std::fs::write(&exe, b"x").unwrap();
        let ok = folder("ok", "adopted", &exe);
        assert_eq!(
            run(&home, &ok, NOW).unwrap(),
            "registered vulpes 0.1.0 (installed separately)"
        );
        let reg = Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.contains_key("vulpes"));
        drop(reg);
        // Again: an update, not an error.
        assert!(run(&home, &ok, NOW).is_ok());
        // The program isn't there.
        let gone = folder("gone", "adopted", &install.join("missing.exe"));
        let e = run(&temp("home2"), &gone, NOW).unwrap_err();
        assert!(e.starts_with("not registered: "), "{e}");
        // Not an adopted manifest.
        let svc = folder("svc", "service", &exe);
        assert!(run(&temp("home3"), &svc, NOW).is_err());
        // No manifest in the folder.
        let e = run(&temp("home4"), &temp("empty"), NOW).unwrap_err();
        assert!(e.contains("module.toml"), "{e}");
    }
}
