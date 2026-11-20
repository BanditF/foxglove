// SPDX-License-Identifier: MIT
//! Dev folders, the take-back paths: an enrolled app that is removed and adopted again is a
//! new app (no grants, no enrolment, no old chain), and a listed folder whose drive stops being
//! this PC's own (network, virtual, not fixed, unreadable) stops carrying. Fake probes only.
//! Window-free.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hideout_registry::dev_folder::{DevProbes, RowProblem};
use hideout_registry::drive::{BusType, DriveProbe, NotLocalKind};
use hideout_registry::inventory::IdentityClass;
use hideout_registry::keyboard::VoidReason;
use hideout_registry::sync_root::{SyncAnswer, SyncProbe};
use hideout_registry::Registry;

const NOW: &str = "2026-11-20T12:00:00Z";
const ID: &str = "foxglove";

fn temp(name: &str) -> PathBuf {
    // Not %TEMP%: a dev folder must not be writable by other accounts.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("hideout-devreadd-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Fx {
    home: PathBuf,
    /// Where `module.toml` is.
    folder: PathBuf,
    exe: PathBuf,
    reg: Registry,
}

/// An adopted app at `<out>\foxglove.exe`, with `out` marked as a dev folder.
fn fx(name: &str) -> Fx {
    let out = temp(&format!("{name}-out"));
    let exe = out.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = [\"{}\"]\napi = {{ foxglove = \"1.0\" }}\n\
             permissions = [\"hub.notify\", \"ui.window\"]\n",
            exe.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.add_dev_folder(&out.display().to_string()).unwrap();
    Fx {
        home,
        folder,
        exe,
        reg,
    }
}

fn allow(reg: &mut Registry, by: &str) {
    let list = reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    reg.approve_listed(ID, list.code(), &all, &[], NOW, by, false)
        .unwrap();
}

fn rebuild(exe: &Path, bytes: &[u8]) {
    std::fs::write(exe, bytes).unwrap();
}

#[test]
fn an_enrolled_app_removed_and_adopted_again_is_a_new_app() {
    let mut f = fx("readd");
    allow(&mut f.reg, "dialog");
    f.reg.set_enabled(ID, true).unwrap();
    rebuild(&f.exe, b"build 2");
    f.reg.pre_launch(ID).unwrap();
    assert_eq!(f.reg.void_if_changed(ID, NOW).unwrap(), None);
    let m = &f.reg.inventory.modules[ID];
    assert!(m.identity.is_some() && !m.granted.is_empty());
    assert_eq!(m.bound.as_ref().unwrap().attested.len(), 1);
    assert!(f.reg.dev_folder_of(ID).is_some());

    f.reg.remove(ID).unwrap();
    // The folder stays listed: it is the user's choice about a place, not about this app.
    assert_eq!(f.reg.dev_folders().len(), 1);
    assert!(f.reg.dev_folder_of(ID).is_none());

    // Adopted again from the same place, on a registry read from disk.
    let mut reg = Registry::open(&f.home, true).unwrap();
    reg.register_adopted(&f.folder, NOW).unwrap();
    let m = &reg.inventory.modules[ID];
    assert!(m.granted.is_empty(), "a grant came back: {:?}", m.granted);
    assert!(m.identity.is_none(), "still enrolled: {:?}", m.identity);
    assert!(m.bound.is_none() && m.last_choice.is_empty());
    assert!(reg.dev_folder_of(ID).is_none());
    assert_eq!(reg.dev_folders().len(), 1);

    // A rebuild before any Allow carries nothing (there is nothing to carry).
    rebuild(&f.exe, b"build 3");
    assert!(reg.inventory.modules[ID].granted.is_empty());
    assert!(reg.dev_folder_of(ID).is_none());

    // A command-line Allow grants and does not enrol; only the window does, with a new chain.
    allow(&mut reg, "cli");
    assert!(reg.inventory.modules[ID].identity.is_none());
    assert!(reg.dev_folder_of(ID).is_none());
    rebuild(&f.exe, b"build 4");
    // Not enrolled, so the rebuild is not carried: the launch check voids and asks again.
    assert_eq!(
        reg.void_if_changed(ID, NOW).unwrap(),
        Some(VoidReason::Changed)
    );
    assert!(reg.inventory.modules[ID].granted.is_empty());
    allow(&mut reg, "dialog");
    let m = &reg.inventory.modules[ID];
    let id = m.identity.as_ref().expect("enrolled by the dialog");
    assert!(matches!(
        &id.class,
        IdentityClass::DevFolder { folder: Some(_) }
    ));
    assert!(
        m.bound.as_ref().unwrap().attested.is_empty(),
        "the removed app's chain came back"
    );
    assert!(reg.dev_folder_of(ID).is_some());
}

/// One answer for every drive.
#[derive(Clone, Copy)]
struct Drives {
    fixed: bool,
    bus: Option<BusType>,
    remote: bool,
}
impl DriveProbe for Drives {
    fn is_fixed(&self, _: &str) -> bool {
        self.fixed
    }
    fn bus_type(&self, _: &str) -> Option<BusType> {
        self.bus
    }
    fn is_remote(&self, _: &str) -> bool {
        self.remote
    }
}

struct Local;
impl SyncProbe for Local {
    fn check(&self, _: &str) -> SyncAnswer {
        SyncAnswer::Local
    }
}

fn probes(fixed: bool, bus: Option<BusType>, remote: bool) -> DevProbes {
    DevProbes {
        drives: Arc::new(Drives { fixed, bus, remote }),
        sync: Arc::new(Local),
    }
}

#[test]
fn a_listed_folder_whose_drive_is_no_longer_the_pcs_own_stops_carrying() {
    let mut f = fx("drive");
    allow(&mut f.reg, "dialog");
    let internal = || probes(true, Some(BusType::Nvme), false);
    f.reg.set_dev_probes(internal());
    assert!(f.reg.dev_folder_of(ID).is_some());
    assert_eq!(f.reg.dev_folder_rows()[0].problem, None);
    rebuild(&f.exe, b"build 2");

    let cases = [
        // A virtual disk (VHD), fixed or not.
        (
            probes(true, Some(BusType::Virtual), false),
            NotLocalKind::VirtualOrUnknown,
        ),
        // A bus that can't be read.
        (probes(true, None, false), NotLocalKind::VirtualOrUnknown),
        // An internal bus on a drive that is not fixed.
        (
            probes(false, Some(BusType::Sata), false),
            NotLocalKind::VirtualOrUnknown,
        ),
        // A mapped network drive, whatever else it says.
        (
            probes(true, Some(BusType::Nvme), true),
            NotLocalKind::Network,
        ),
        (probes(false, None, true), NotLocalKind::Network),
    ];
    for (i, (p, want)) in cases.into_iter().enumerate() {
        f.reg.set_dev_probes(p);
        assert_eq!(
            f.reg.dev_folder_rows()[0].problem,
            Some(RowProblem::NotLocal(want)),
            "case {i}"
        );
        assert!(f.reg.dev_folder_of(ID).is_none(), "case {i}");
        // The rebuild is not carried while the folder does not qualify.
        assert_eq!(
            f.reg.binding_void(ID),
            Some(VoidReason::Changed),
            "case {i}"
        );
        // Nothing was written by looking: the drive back as the PC's own carries again.
        f.reg.set_dev_probes(internal());
        assert_eq!(f.reg.binding_void(ID), None, "case {i}");
        assert!(f.reg.dev_folder_of(ID).is_some(), "case {i}");
    }

    // Once the void is written (a launch check on the bad drive), it stays: the drive coming
    // back does not restore the grants; the user allows again.
    f.reg
        .set_dev_probes(probes(true, Some(BusType::Virtual), false));
    assert_eq!(
        f.reg.void_if_changed(ID, NOW).unwrap(),
        Some(VoidReason::Changed)
    );
    f.reg.set_dev_probes(internal());
    assert!(f.reg.binding_void(ID).is_some());
    assert!(f.reg.dev_folder_of(ID).is_none());
    let again = Registry::open(&f.home, true).unwrap();
    assert!(again.binding_void(ID).is_some());
}
