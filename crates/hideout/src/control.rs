// SPDX-License-Identifier: MIT
//! The hub's control pipe and `hideout.exe --modules <step> <id> ...` (#1178,
//! docs/hideout-permission-dialog-ux.md section 7). `permissions` is read-only and works
//! without a hub; `grant`, `deny` and `revoke` are requests over `\\.\pipe\hideout-control-<user>`
//! so the router widens from the user's act and narrows first.
//!
//! Who may ask: the hub checks the client process is **not** inside any module's Job Object
//! (a module and its children can never grant, `permission.caller`) and that its image is the
//! hub's own `hideout.exe`. Exit codes: 0 done, 1 refused (error id printed), 2 usage, 3 needs
//! approval or the list code is missing or stale, 4 busy, 5 the hub isn't running.

use std::path::Path;

use hideout_registry::permission::{ItemState, Listing};
use hideout_registry::Registry;

pub const DONE: i32 = 0;
pub const REFUSED: i32 = 1;
pub const USAGE: i32 = 2;
pub const NEEDS_OK: i32 = 3;
pub const BUSY: i32 = 4;
pub const NO_HUB: i32 = 5;

/// `\\.\pipe\hideout-control-<user><suffix>`.
pub fn pipe_name(paths: &hideout_shell::paths::HubPaths) -> String {
    format!(
        r"\\.\pipe\hideout-control-{}{}",
        hideout_router::server::user(),
        paths.instance_suffix()
    )
}

/// What `permissions` prints: every item with its group, raw string, state and plain line, the
/// program, and the list code.
pub fn render(l: &Listing, granted: &[String], declined: &[String]) -> String {
    let mut s = format!(
        "{} {} asks for {}.\nProgram: {}  sha256 {}\n",
        l.name,
        l.version,
        l.items.len(),
        if l.exe_path.is_empty() {
            "(installed by Hideout)"
        } else {
            &l.exe_path
        },
        l.exe_sha256
    );
    for i in &l.items {
        let state = if granted.contains(&i.raw) {
            "allowed"
        } else if i.state != ItemState::Asked {
            "can't be allowed"
        } else if declined.contains(&i.raw) {
            "off"
        } else {
            "waiting"
        };
        s.push_str(&format!(
            "  {:<28} {:<36} {:<16} {}\n",
            i.group.heading(),
            i.raw,
            state,
            i.line
        ));
    }
    s.push_str(&format!(
        "List code {}. To allow: hideout modules grant {} --all-listed --listed {}\n",
        l.code(),
        l.id,
        l.code()
    ));
    s
}

fn usage(why: &str) -> (i32, String) {
    (USAGE, format!("hideout: {why}\n"))
}

/// `permissions <id>`: read-only, no hub needed.
pub fn permissions(home: &Path, id: &str) -> (i32, String) {
    let reg = match Registry::open_from_env(home) {
        Ok(r) => r,
        Err(e) => return (REFUSED, format!("hideout: registry not read ({e:?})\n")),
    };
    match reg.permission_list(id) {
        Ok(l) => {
            let m = &reg.inventory.modules[id];
            (DONE, render(&l, &m.granted, &m.declined_permissions))
        }
        Err(e) => (REFUSED, format!("hideout: {id}: {e:?}\n")),
    }
}

fn exit_for(e: &str) -> i32 {
    if e == "permission.list_changed" {
        NEEDS_OK
    } else if e.contains("lock") {
        BUSY
    } else {
        REFUSED
    }
}

/// Runs one request line (`grant|deny|revoke <id> ...`) in the hub. `caller_ok` is the result
/// of the caller check; a caller that may not grant is refused with `permission.caller` and
/// nothing is read or written.
pub fn handle(
    home: &Path,
    hub: &hideout_router::server::Hub,
    caller_ok: bool,
    line: &str,
) -> (i32, String) {
    if !caller_ok {
        log::warn!(
            "permissions: control pipe: refused a client inside a module job or not hideout"
        );
        return (
            REFUSED,
            "permission.caller: this caller may not change permissions\n".into(),
        );
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let [step, id, rest @ ..] = words.as_slice() else {
        return usage("expected `<grant|deny|revoke> <id> ...`");
    };
    let (mut code, mut all, mut keyboard, mut all_items) = (None, false, false, false);
    let mut items: Vec<String> = Vec::new();
    let mut it = rest.iter();
    while let Some(w) = it.next() {
        match *w {
            "--listed" => code = it.next().map(|c| c.to_string()),
            "--all-listed" => all = true,
            "--all" => all_items = true,
            "--keyboard" => keyboard = true,
            o if o.starts_with("--") => return usage(&format!("unknown option {o}")),
            p => items.push(p.to_string()),
        }
    }
    match *step {
        "revoke" => {
            if !all_items && items.is_empty() && !keyboard {
                return usage("revoke needs items, --all or --keyboard");
            }
            let mut out = String::new();
            if all_items || !items.is_empty() {
                let which = if all_items {
                    None
                } else {
                    Some(items.as_slice())
                };
                match hideout_supervisor::driver::revoke_items_in(home, hub, id, which) {
                    Ok(left) => {
                        out.push_str(&format!("turned off; still allowed: {}\n", left.len()))
                    }
                    Err(e) => return (exit_for(&e), format!("hideout: {e}\n")),
                }
            }
            if keyboard {
                if let Err(e) = crate::keyboard_actions::turn_off(home, hub, id) {
                    return (REFUSED, format!("hideout: {e}\n"));
                }
                out.push_str("keyboard grant turned off\n");
            }
            (DONE, out)
        }
        "grant" | "deny" => {
            let reg = match Registry::open_from_env(home) {
                Ok(r) => r,
                Err(e) => return (REFUSED, format!("hideout: registry not read ({e:?})\n")),
            };
            let list = match reg.permission_list(id) {
                Ok(l) => l,
                Err(e) => return (REFUSED, format!("hideout: {id}: {e:?}\n")),
            };
            let Some(code) = code else {
                let m = &reg.inventory.modules[*id];
                let mut text = render(&list, &m.granted, &m.declined_permissions);
                text.push_str("Not allowed: --listed <code> is needed.\n");
                return (NEEDS_OK, text);
            };
            if keyboard && *step == "deny" {
                return usage("--keyboard is for grant");
            }
            let named: Vec<String> = if all {
                list.grantable().map(|i| i.raw.clone()).collect()
            } else if items.is_empty() {
                return usage("name the items or use --all-listed");
            } else {
                items
            };
            let (chosen, declined) = if *step == "grant" {
                (named, Vec::new())
            } else {
                (Vec::new(), named)
            };
            let r = hideout_supervisor::driver::grant_listed_in(
                home, hub, id, &code, &chosen, &declined, "cli",
            );
            match r {
                Ok(o) => {
                    let mut s = String::new();
                    for g in &o.granted {
                        s.push_str(&format!("allowed   {g}\n"));
                    }
                    for d in &o.declined {
                        s.push_str(&format!("declined  {d}\n"));
                    }
                    for (r, e) in &o.refused {
                        s.push_str(&format!("refused   {r} ({})\n", e.id()));
                    }
                    s.push_str(&format!("bound to {}\n", o.bound_path));
                    if keyboard {
                        match crate::keyboard_actions::dialog_path(home, id)
                            .and_then(|p| crate::keyboard_actions::allow(home, hub, id, &p))
                        {
                            Ok(()) => s.push_str("allowed   keyboard grant\n"),
                            Err(e) => s.push_str(&format!("refused   keyboard grant ({e})\n")),
                        }
                    }
                    let nothing = o.granted.is_empty() && !chosen.is_empty();
                    (if nothing { REFUSED } else { DONE }, s)
                }
                Err(e) if e == "permission.list_changed" => (
                    NEEDS_OK,
                    "Not allowed: the list changed since you read it (permission.list_changed). Read it again.\n"
                        .into(),
                ),
                Err(e) => (exit_for(&e), format!("hideout: {e}\n")),
            }
        }
        other => usage(&format!("unknown step {other}")),
    }
}

/// Whether a connecting process may grant: not inside any module's job, and the hub's own image.
pub fn caller_ok(sup: &hideout_supervisor::driver::Supervisor, pid: u32) -> bool {
    if sup.in_module_job(pid) {
        return false;
    }
    let me = std::env::current_exe().ok();
    let theirs = hideout_supervisor::win::Process::open(pid)
        .and_then(|p| p.image())
        .ok();
    match (me, theirs) {
        (Some(a), Some(b)) => {
            hideout_registry::keyboard::same_path(&a.to_string_lossy(), &b.to_string_lossy())
        }
        _ => false,
    }
}

/// Serves the control pipe from a thread of its own, one request per connection.
pub fn serve(
    name: &str,
    home: std::path::PathBuf,
    hub: hideout_router::server::Hub,
    sup: hideout_supervisor::driver::Supervisor,
) -> std::io::Result<()> {
    use foxglove_plugin::channel::ServerPipe;
    use std::io::{BufRead, BufReader, Read, Write};
    let first = ServerPipe::create(name, true)?;
    let name = name.to_string();
    std::thread::Builder::new()
        .name("hideout-control".into())
        .spawn(move || {
            // The next instance of the pipe exists before this one is served, so a client that
            // connects while a request is handled finds the pipe, not "no hub".
            let mut pipe = first;
            loop {
                let conn = pipe.accept();
                let next = ServerPipe::create(&name, false);
                if let Ok(mut conn) = conn {
                    let ok = conn.client_pid().is_ok_and(|p| caller_ok(&sup, p));
                    let mut line = String::new();
                    let _ = BufReader::new(conn.clone()).take(4096).read_line(&mut line);
                    let (code, text) = handle(&home, &hub, ok, &line);
                    let _ = conn.write_all(
                        format!(
                            "{code}
{text}"
                        )
                        .as_bytes(),
                    );
                }
                match next {
                    Ok(p) => pipe = p,
                    Err(e) => {
                        log::error!("control: {name}: {e}");
                        return;
                    }
                }
            }
        })?;
    Ok(())
}

/// The client: sends one request line, returns (exit code, text). No hub = exit 5.
pub fn request(pipe: &str, line: &str) -> (i32, String) {
    use std::io::{Read, Write};
    let Ok(mut c) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)
    else {
        return (
            NO_HUB,
            "hideout: the hub isn't running; nothing changed\n".into(),
        );
    };
    let _ = c.write_all(format!("{line}\n").as_bytes());
    let mut reply = String::new();
    let _ = c.read_to_string(&mut reply);
    match reply.split_once('\n') {
        Some((code, text)) => (code.parse().unwrap_or(REFUSED), text.to_string()),
        None => (REFUSED, "hideout: no answer\n".into()),
    }
}

/// `hideout.exe --modules <step> <id> ...`: `None` for any other command line.
pub fn client(
    paths: &hideout_shell::paths::HubPaths,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    if args.next()? != "--modules" {
        return None;
    }
    let rest: Vec<String> = args.map(|a| a.to_string_lossy().into_owned()).collect();
    let (code, text) = match rest.as_slice() {
        [s, id, flags @ ..] if s == "permissions" => {
            let (c, t) = permissions(&paths.root, id);
            if c == DONE && flags.iter().any(|f| f == "--json") {
                json(&paths.root, id)
            } else {
                (c, t)
            }
        }
        [s, _, ..] if matches!(s.as_str(), "grant" | "deny" | "revoke") => {
            request(&pipe_name(paths), &rest.join(" "))
        }
        _ => usage("usage: --modules permissions|grant|deny|revoke <id> ..."),
    };
    print!("{text}");
    Some(code)
}

fn json(home: &Path, id: &str) -> (i32, String) {
    let Ok(reg) = Registry::open_from_env(home) else {
        return (REFUSED, "hideout: registry not read\n".into());
    };
    let Ok(l) = reg.permission_list(id) else {
        return (REFUSED, "hideout: not installed\n".into());
    };
    let m = &reg.inventory.modules[id];
    let v = serde_json::json!({"ok": true, "list_hash": l.list_hash, "listing": l,
        "granted": m.granted, "declined": m.declined_permissions});
    (DONE, format!("{v}\n"))
}

#[cfg(test)]
mod tests {
    //! Live tests, window-free: a lab hub folder, the real control pipe served from a thread, the
    //! test process as the CLI (same image as the hub, outside every module job), and a real
    //! PowerShell as a client the pipe must refuse: once outside the hub, once as a module the
    //! supervisor runs inside its Job Object.
    use super::*;
    use hideout_router::{Router, Settings};
    use hideout_supervisor::driver::Supervisor;
    use std::path::PathBuf;
    use std::sync::mpsc;

    const NOW: &str = "2026-11-13T00:00:00Z";

    struct Fx {
        paths: hideout_shell::paths::HubPaths,
        hub: hideout_router::server::Hub,
        sup: Supervisor,
        id: String,
        pipe: String,
        base: PathBuf,
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    /// An adopted module `example.lab.ctl<name>` asking for `hub.notify` and `ui.window`, run by
    /// `run` (`{base}` is the fixture's folder); its hub folder served on a control pipe of its
    /// own.
    fn fx(name: &str, run: &[String]) -> Fx {
        std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
        let base = std::env::temp_dir().join(format!("hideout-ctl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let folder = base.join("manifest");
        std::fs::create_dir_all(&folder).unwrap();
        let id = format!("example.lab.ctl{name}");
        let b = base.display().to_string();
        let run: Vec<String> = run
            .iter()
            .map(|a| format!("\"{}\"", a.replace("{base}", &b).replace('\\', "/")))
            .collect();
        std::fs::write(
            folder.join("module.toml"),
            format!(
                "id = \"{id}\"\nname = \"Ctl\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
                 family = \"arete\"\nhub = \"1.0\"\nrun = [{}]\napi = {{ arete = \"1.0\" }}\n\
                 permissions = [\"hub.notify\", \"ui.window\"]\n",
                run.join(", ")
            ),
        )
        .unwrap();
        let root = base.join("home");
        let mut reg = Registry::open(&root, true).unwrap();
        reg.register_adopted(&folder, NOW).unwrap();
        let paths = hideout_shell::paths::HubPaths {
            root: root.clone(),
            overridden: true,
        };
        let router = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
        let (events, _rx) = mpsc::channel();
        let hub = hideout_router::server::Hub::new(router, events);
        hub.set_folder_suffix(&hideout_router::server::folder_suffix(&root));
        let sup = Supervisor::start(&root, hub.clone());
        let pipe = pipe_name(&paths);
        serve(&pipe, root, hub.clone(), sup.clone()).expect("control pipe");
        Fx {
            paths,
            hub,
            sup,
            id,
            pipe,
            base,
        }
    }

    fn windir() -> String {
        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
    }

    fn ping() -> Vec<String> {
        vec![
            format!(r"{}\System32\PING.EXE", windir()),
            "-n".into(),
            "60".into(),
            "127.0.0.1".into(),
        ]
    }

    fn state(f: &Fx) -> (Vec<String>, Vec<String>) {
        let r = Registry::open_from_env(&f.paths.root).unwrap();
        let m = &r.inventory.modules[&f.id];
        (m.granted.clone(), m.pending_permissions.clone())
    }

    fn code(f: &Fx) -> String {
        Registry::open_from_env(&f.paths.root)
            .unwrap()
            .permission_list(&f.id)
            .unwrap()
            .code()
            .to_string()
    }

    #[test]
    fn permissions_reads_without_a_hub_and_names_the_list_code() {
        let f = fx("read", &ping());
        let (c, text) = permissions(&f.paths.root, &f.id);
        assert_eq!(c, DONE, "{text}");
        assert!(
            text.contains("hub.notify") && text.contains("ui.window"),
            "{text}"
        );
        assert!(text.contains(&format!("List code {}", code(&f))), "{text}");
        assert!(text.contains("waiting"), "{text}");
        assert_eq!(permissions(&f.paths.root, "no.such.module").0, REFUSED);
        let (c, j) = json(&f.paths.root, &f.id);
        assert_eq!(c, DONE);
        assert!(j.contains("\"list_hash\""), "{j}");
        assert!(state(&f).0.is_empty());
    }

    #[test]
    fn the_pipe_grants_only_the_listed_code_and_exit_codes_say_what_happened() {
        let f = fx("grant", &ping());
        // No code: the list is printed, exit 3, nothing granted.
        let (c, t) = request(&f.pipe, &format!("grant {} --all-listed", f.id));
        assert_eq!(c, NEEDS_OK, "{t}");
        assert!(t.contains("--listed <code> is needed"), "{t}");
        // A stale code: exit 3, nothing granted.
        let (c, t) = request(
            &f.pipe,
            &format!("grant {} --all-listed --listed {}", f.id, "0".repeat(16)),
        );
        assert_eq!(c, NEEDS_OK, "{t}");
        assert!(t.contains("permission.list_changed"), "{t}");
        assert!(state(&f).0.is_empty());
        // Usage mistakes: exit 2, nothing granted.
        let good = code(&f);
        for bad in [
            format!("grant {} --listed {good}", f.id),
            format!("grant {} hub.notify --bogus --listed {good}", f.id),
            format!("deny {} hub.notify --keyboard --listed {good}", f.id),
            format!("frobnicate {}", f.id),
            "grant".to_string(),
            format!("revoke {}", f.id),
        ] {
            let (c, t) = request(&f.pipe, &bad);
            assert_eq!(c, USAGE, "{bad}: {t}");
        }
        assert!(state(&f).0.is_empty());
        // An item that is not in the list: refused (1), nothing granted.
        let (c, t) = request(&f.pipe, &format!("grant {} net --listed {good}", f.id));
        assert_eq!(c, REFUSED, "{t}");
        assert!(state(&f).0.is_empty());
        // The shown list, one item: it is granted and the other stays pending.
        let (c, t) = request(
            &f.pipe,
            &format!("grant {} hub.notify --listed {good}", f.id),
        );
        assert_eq!(c, DONE, "{t}");
        assert!(
            t.contains("allowed   hub.notify") && t.contains("bound to"),
            "{t}"
        );
        let (granted, pending) = state(&f);
        assert_eq!(granted, ["hub.notify"]);
        assert_eq!(pending, ["ui.window"]);
        // The router follows the file at once.
        assert_eq!(f.hub.read(|r| r.module_grants(&f.id)), ["hub.notify"]);
        // Revoke narrows and needs no code.
        let (c, t) = request(&f.pipe, &format!("revoke {} --all", f.id));
        assert_eq!(c, DONE, "{t}");
        assert!(state(&f).0.is_empty());
        assert!(f.hub.read(|r| r.module_grants(&f.id)).is_empty());
    }

    #[test]
    fn no_hub_is_exit_5_and_the_cli_words_are_parsed() {
        let f = fx("cli", &ping());
        let gone = r"\\.\pipe\hideout-control-nobody-here-1178";
        let (c, t) = request(gone, "grant x --all-listed --listed 0");
        assert_eq!(c, NO_HUB, "{t}");
        assert!(t.contains("nothing changed"), "{t}");

        let os = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        // Not a `--modules` command line: not ours.
        assert_eq!(client(&f.paths, os(&["--adopt", "x"]).into_iter()), None);
        assert_eq!(client(&f.paths, os(&[]).into_iter()), None);
        // Unknown step and a bare `--modules`: usage.
        assert_eq!(
            client(&f.paths, os(&["--modules", "frobnicate", "x"]).into_iter()),
            Some(USAGE)
        );
        assert_eq!(
            client(&f.paths, os(&["--modules"]).into_iter()),
            Some(USAGE)
        );
        // permissions works through the CLI front end.
        assert_eq!(
            client(
                &f.paths,
                os(&["--modules", "permissions", &f.id]).into_iter()
            ),
            Some(DONE)
        );
        assert_eq!(
            client(
                &f.paths,
                os(&["--modules", "permissions", "nope"]).into_iter()
            ),
            Some(REFUSED)
        );
        // grant and revoke through the CLI go over the served pipe.
        let good = code(&f);
        assert_eq!(
            client(
                &f.paths,
                os(&[
                    "--modules",
                    "grant",
                    &f.id,
                    "--all-listed",
                    "--listed",
                    &good
                ])
                .into_iter()
            ),
            Some(DONE)
        );
        assert_eq!(state(&f).0.len(), 2);
        assert_eq!(
            client(
                &f.paths,
                os(&["--modules", "revoke", &f.id, "ui.window"]).into_iter()
            ),
            Some(DONE)
        );
        assert_eq!(state(&f).0, ["hub.notify"]);
    }

    #[test]
    fn a_caller_that_may_not_grant_is_refused_before_anything_is_read() {
        let f = fx("caller", &ping());
        let good = code(&f);
        let (c, t) = handle(
            &f.paths.root,
            &f.hub,
            false,
            &format!("grant {} --all-listed --listed {good}", f.id),
        );
        assert_eq!(c, REFUSED);
        assert!(t.starts_with("permission.caller"), "{t}");
        assert!(state(&f).0.is_empty());
        // Even an unparsable line is answered with the caller refusal, not usage.
        assert!(handle(&f.paths.root, &f.hub, false, "???")
            .1
            .starts_with("permission.caller"));
    }

    /// A PowerShell script that sends `revoke <id> --all` over the control pipe and writes the
    /// whole reply to a file; returns (script, reply file). Written after the fixture exists.
    fn script(f: &Fx) -> (PathBuf, PathBuf) {
        let out = f.base.join("reply.txt");
        let ps1 = f.base.join("client.ps1");
        let pipe = f.pipe.rsplit('\\').next().unwrap();
        std::fs::write(
            &ps1,
            format!(
                "$p = New-Object System.IO.Pipes.NamedPipeClientStream('.', '{pipe}', 'InOut')\n\
                 $p.Connect(10000)\n\
                 $w = New-Object System.IO.StreamWriter($p)\n\
                 $w.WriteLine('revoke {} --all')\n$w.Flush()\n\
                 $r = New-Object System.IO.StreamReader($p)\n\
                 [System.IO.File]::WriteAllText('{}', $r.ReadToEnd())\n",
                f.id,
                out.display()
            ),
        )
        .unwrap();
        (ps1, out)
    }

    fn powershell() -> String {
        format!(
            r"{}\System32\WindowsPowerShell\v1.0\powershell.exe",
            windir()
        )
    }

    fn grant_all(f: &Fx) {
        let good = code(f);
        let (c, t) = request(
            &f.pipe,
            &format!("grant {} --all-listed --listed {good}", f.id),
        );
        assert_eq!(c, DONE, "{t}");
        assert_eq!(state(f).0.len(), 2);
    }

    #[test]
    fn a_foreign_process_outside_the_hub_cannot_use_the_pipe() {
        let f = fx("foreign", &ping());
        grant_all(&f);
        let (ps1, out) = script(&f);
        let status = std::process::Command::new(powershell())
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&ps1)
            .status()
            .expect("powershell");
        assert!(status.success());
        let reply = std::fs::read_to_string(&out).expect("no reply");
        assert!(reply.starts_with("1\npermission.caller"), "{reply}");
        assert_eq!(state(&f).0.len(), 2, "a refused caller changed a grant");
    }

    #[test]
    fn a_module_running_in_its_job_cannot_use_the_pipe() {
        // The module itself is the client: a PowerShell the supervisor starts in its Job Object.
        let f = fx(
            "job",
            &[
                powershell(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                "{base}/client.ps1".into(),
            ],
        );
        let (_, out) = script(&f);
        grant_all(&f);
        f.sup.turn_on(&f.id).expect("turn on");
        let pid = f.sup.pid(&f.id).expect("module not started");
        assert!(
            f.sup.in_module_job(pid),
            "the module is not in its Job Object"
        );
        let p = hideout_supervisor::win::Process::open(pid).unwrap();
        let _ = p.wait();
        f.sup.turn_off(&f.id);
        let reply = std::fs::read_to_string(&out).expect("the module got no reply");
        assert!(reply.starts_with("1\npermission.caller"), "{reply}");
        assert_eq!(state(&f).0.len(), 2, "a module changed its own grants");
    }

    #[test]
    fn this_process_is_not_in_a_module_job_and_is_the_hubs_image() {
        let f = fx("self", &ping());
        assert!(!f.sup.in_module_job(std::process::id()));
        assert!(caller_ok(&f.sup, std::process::id()));
        // A process that is not hideout.exe (any other image) may not grant.
        let mut other = std::process::Command::new(windir() + r"\System32\PING.EXE")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert!(!caller_ok(&f.sup, other.id()));
        let _ = other.kill();
        let _ = other.wait();
    }
}
