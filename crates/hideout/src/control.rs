// SPDX-License-Identifier: MIT
//! The hub's control pipe and `hideout.exe --modules <step> <id> ...` (#1178,
//! docs/hideout-permission-dialog-ux.md section 7). `permissions` is read-only and works
//! without a hub; `grant`, `deny` and `revoke` are requests over `\\.\pipe\hideout-control-<user>`
//! so the router widens from the user's act and narrows first.
//!
//! `grant` never widens by itself (#1183): a request with the current list code is queued
//! (`grant_queue`) and the user confirms it in the Modules page's permission dialog; exit 3.
//! `deny` and `revoke` only narrow and apply at once. A lab hub (`lab_control::enabled`, a folder
//! of its own) may apply a grant directly so lab runs stay headless: the `auto_confirm` seam.
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
    queue: &crate::grant_queue::GrantQueue,
    auto_confirm: bool,
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
            if *step == "grant" && !auto_confirm {
                return queue_grant(home, queue, &list, &code, named, keyboard);
            }
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

/// A grant from the command line: checked against the list as it is now, then queued for the
/// user's confirmation. Nothing is written and the router is not touched.
fn queue_grant(
    home: &Path,
    queue: &crate::grant_queue::GrantQueue,
    list: &Listing,
    code: &str,
    named: Vec<String>,
    keyboard: bool,
) -> (i32, String) {
    if keyboard {
        return (
            REFUSED,
            "hideout: --keyboard can only be allowed in the Hideout window
"
            .into(),
        );
    }
    if code != list.code() {
        return (
            NEEDS_OK,
            "Not allowed: the list changed since you read it (permission.list_changed). Read it again.
"
                .into(),
        );
    }
    let grantable: Vec<&str> = list.grantable().map(|i| i.raw.as_str()).collect();
    if let Some(bad) = named.iter().find(|n| !grantable.contains(&n.as_str())) {
        return (
            REFUSED,
            format!(
                "hideout: {bad} can't be allowed
"
            ),
        );
    }
    let mut named = named;
    named.sort();
    named.dedup();
    let n = named.len();
    queue.submit(crate::grant_queue::Queued {
        home: home.to_path_buf(),
        id: list.id.clone(),
        code: code.to_string(),
        items: named,
    });
    (
        NEEDS_OK,
        format!(
            "Queued: {n} item(s) for {}. Nothing is allowed yet: confirm in Hideout, on the Modules page.",
            list.name
        ),
    )
}

/// Whether a connecting process may grant: not inside any module's job (live, or retired with
/// processes still in it), and the hub's own image. The process is opened once; the same handle
/// serves both checks.
pub fn caller_ok(sup: &hideout_supervisor::driver::Supervisor, pid: u32) -> bool {
    let Ok(p) = hideout_supervisor::win::Process::open(pid) else {
        return false;
    };
    if sup.process_in_module_job(&p) {
        return false;
    }
    let me = std::env::current_exe().ok();
    match (me, p.image().ok()) {
        (Some(a), Some(b)) => {
            hideout_registry::keyboard::same_path(&a.to_string_lossy(), &b.to_string_lossy())
        }
        _ => false,
    }
}

/// How long a permitted client has to send its request line.
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Reads one request line (at most 4096 bytes) before `READ_DEADLINE`; `None` when the client
/// said nothing in time or closed without a line.
fn read_line_before(conn: &foxglove_plugin::channel::PipeConn) -> Option<String> {
    let end = std::time::Instant::now() + READ_DEADLINE;
    let mut line = Vec::new();
    let mut buf = [0u8; 256];
    while line.len() < 4096 && !line.contains(&b'\n') {
        match conn.read_before(&mut buf, end) {
            Ok(0) | Err(_) => break,
            Ok(n) => line.extend_from_slice(&buf[..n]),
        }
    }
    if !line.contains(&b'\n') && line.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// Serves the control pipe from a thread of its own, one request per connection.
pub fn serve(
    name: &str,
    home: std::path::PathBuf,
    hub: hideout_router::server::Hub,
    sup: hideout_supervisor::driver::Supervisor,
    queue: crate::grant_queue::GrantQueue,
    auto_confirm: bool,
) -> std::io::Result<()> {
    use foxglove_plugin::channel::ServerPipe;
    use std::io::Write;
    let first = ServerPipe::create(name, true)?;
    let name = name.to_string();
    std::thread::Builder::new()
        .name("hideout-control".into())
        .spawn(move || {
            // The next instance of the pipe exists before this one is even accepted, so a client
            // that connects while a request is handled finds the pipe, not "no hub". Made after
            // the accept it left a gap: a client that connected and hung up before the server
            // thread ran (the accept fails, closing that instance) found no instance at all.
            let mut pipe = first;
            loop {
                let next = ServerPipe::create(&name, false);
                let conn = pipe.accept();
                if let Ok(mut conn) = conn {
                    let ok = conn.client_pid().is_ok_and(|p| caller_ok(&sup, p));
                    // A caller that may not grant is refused without reading its request.
                    let line = if ok {
                        read_line_before(&conn).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let (code, text) = if ok && line.is_empty() {
                        usage("no request line")
                    } else {
                        handle(&home, &hub, ok, &line, &queue, auto_confirm)
                    };
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

/// The client: sends one request line, returns (exit code, text). No hub = exit 5. The server
/// must be this install's `hideout.exe`, same user (#1184): see [`request_to`].
#[cfg(test)]
pub fn request(pipe: &str, line: &str) -> (i32, String) {
    request_to(pipe, line, foxglove_plugin::owner::Expect::HIDEOUT)
}

/// How long a client waits for a busy pipe before giving up (the server takes one request at a
/// time and a grant touches the registry).
const WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// [`request`] against a server that must be `expect`. Before a byte is sent the kernel is
/// asked who serves the pipe (`GetNamedPipeServerProcessId`, on the connection the request then
/// uses) and the process must be that image, in this program's install folder, same user, Medium
/// integrity or above, no AppContainer (`foxglove_plugin::owner`). Anything else is a squatter:
/// nothing is sent and the exit code is 1 (`hideout.pipe.foreign`), so the grant request (the
/// module id and list code) never reaches it.
pub fn request_to(pipe: &str, line: &str, expect: foxglove_plugin::owner::Expect) -> (i32, String) {
    use foxglove_plugin::owner::{open_verified, Probe};
    use std::io::{Read, Write};
    let mut c = match open_verified(pipe, expect, WAIT) {
        Ok(c) => c,
        Err(Probe::Foreign { why, .. }) if why.starts_with("every instance busy") => {
            return (BUSY, "hideout: the hub is busy; nothing changed\n".into());
        }
        Err(Probe::Foreign { why, .. }) => {
            return (
                REFUSED,
                format!(
                    "hideout.pipe.foreign\nhideout: something other than this hub answers on \
                     the control pipe ({why}); nothing was sent\n"
                ),
            );
        }
        Err(_) => {
            return (
                NO_HUB,
                "hideout: the hub isn't running; nothing changed\n".into(),
            );
        }
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
    args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    client_as(paths, args, foxglove_plugin::owner::Expect::HIDEOUT)
}

/// [`client`] with the server identity to require (tests expect their own binary).
fn client_as(
    paths: &hideout_shell::paths::HubPaths,
    mut args: impl Iterator<Item = std::ffi::OsString>,
    expect: foxglove_plugin::owner::Expect,
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
            request_to(&pipe_name(paths), &rest.join(" "), expect)
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
        queue: crate::grant_queue::GrantQueue,
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
        fx_with(name, run, true)
    }

    /// [`fx`] with the `auto_confirm` seam off: a hub as the user runs it, where a CLI grant is
    /// queued.
    fn fx_user(name: &str, run: &[String]) -> Fx {
        fx_with(name, run, false)
    }

    fn fx_with(name: &str, run: &[String], auto_confirm: bool) -> Fx {
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
        let queue = crate::grant_queue::GrantQueue::default();
        serve(
            &pipe,
            root,
            hub.clone(),
            sup.clone(),
            queue.clone(),
            auto_confirm,
        )
        .expect("control pipe");
        Fx {
            queue,
            paths,
            hub,
            sup,
            id,
            pipe,
            base,
        }
    }

    /// This test binary's own file name: the identity of the real server in these tests (the
    /// server runs in this process, which is not `hideout.exe`).
    fn own_exe() -> foxglove_plugin::owner::Expect {
        let n = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        foxglove_plugin::owner::Expect {
            exe: Box::leak(n.into_boxed_str()),
        }
    }

    /// The client as the product uses it, but expecting this test binary as the server.
    fn ask(pipe: &str, line: &str) -> (i32, String) {
        request_to(pipe, line, own_exe())
    }

    fn cli(
        paths: &hideout_shell::paths::HubPaths,
        args: impl Iterator<Item = std::ffi::OsString>,
    ) -> Option<i32> {
        client_as(paths, args, own_exe())
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
        let (c, t) = ask(&f.pipe, &format!("grant {} --all-listed", f.id));
        assert_eq!(c, NEEDS_OK, "{t}");
        assert!(t.contains("--listed <code> is needed"), "{t}");
        // A stale code: exit 3, nothing granted.
        let (c, t) = ask(
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
            let (c, t) = ask(&f.pipe, &bad);
            assert_eq!(c, USAGE, "{bad}: {t}");
        }
        assert!(state(&f).0.is_empty());
        // An item that is not in the list: refused (1), nothing granted.
        let (c, t) = ask(&f.pipe, &format!("grant {} net --listed {good}", f.id));
        assert_eq!(c, REFUSED, "{t}");
        assert!(state(&f).0.is_empty());
        // The shown list, one item: it is granted and the other stays pending.
        let (c, t) = ask(
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
        let (c, t) = ask(&f.pipe, &format!("revoke {} --all", f.id));
        assert_eq!(c, DONE, "{t}");
        assert!(state(&f).0.is_empty());
        assert!(f.hub.read(|r| r.module_grants(&f.id)).is_empty());
    }

    #[test]
    fn no_hub_is_exit_5_and_the_cli_words_are_parsed() {
        let f = fx("cli", &ping());
        let gone = r"\\.\pipe\hideout-control-nobody-here-1178";
        let (c, t) = ask(gone, "grant x --all-listed --listed 0");
        assert_eq!(c, NO_HUB, "{t}");
        assert!(t.contains("nothing changed"), "{t}");

        let os = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        // Not a `--modules` command line: not ours.
        assert_eq!(cli(&f.paths, os(&["--adopt", "x"]).into_iter()), None);
        assert_eq!(cli(&f.paths, os(&[]).into_iter()), None);
        // Unknown step and a bare `--modules`: usage.
        assert_eq!(
            cli(&f.paths, os(&["--modules", "frobnicate", "x"]).into_iter()),
            Some(USAGE)
        );
        assert_eq!(cli(&f.paths, os(&["--modules"]).into_iter()), Some(USAGE));
        // permissions works through the CLI front end.
        assert_eq!(
            cli(
                &f.paths,
                os(&["--modules", "permissions", &f.id]).into_iter()
            ),
            Some(DONE)
        );
        assert_eq!(
            cli(
                &f.paths,
                os(&["--modules", "permissions", "nope"]).into_iter()
            ),
            Some(REFUSED)
        );
        // grant and revoke through the CLI go over the served pipe.
        let good = code(&f);
        assert_eq!(
            cli(
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
            cli(
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
            &f.queue,
            true,
        );
        assert_eq!(c, REFUSED);
        assert!(t.starts_with("permission.caller"), "{t}");
        assert!(state(&f).0.is_empty());
        // Even an unparsable line is answered with the caller refusal, not usage.
        assert!(handle(&f.paths.root, &f.hub, false, "???", &f.queue, true)
            .1
            .starts_with("permission.caller"));
        assert!(f.queue.for_home(&f.paths.root).is_empty());
    }

    #[test]
    fn cli_grant_leaves_permission_unchanged_until_confirmed() {
        let f = fx_user("queue", &ping());
        let good = code(&f);
        let (c, t) = ask(
            &f.pipe,
            &format!("grant {} hub.notify --listed {good}", f.id),
        );
        assert_eq!(c, NEEDS_OK, "{t}");
        assert!(
            t.contains("Queued") && t.contains("Nothing is allowed yet"),
            "{t}"
        );
        // Nothing was written and the router did not widen.
        let (granted, pending) = state(&f);
        assert!(granted.is_empty(), "{granted:?}");
        assert_eq!(pending.len(), 2);
        assert!(f.hub.read(|r| r.module_grants(&f.id)).is_empty());
        // The request waits for the user, with exactly the named items and the list code.
        let q = f.queue.get(&f.paths.root, &f.id).expect("queued");
        assert_eq!(
            (q.code.as_str(), q.items.as_slice()),
            (good.as_str(), ["hub.notify".to_string()].as_slice())
        );
        // --all-listed queues too; a newer request replaces the older one.
        let (c, _) = ask(
            &f.pipe,
            &format!("grant {} --all-listed --listed {good}", f.id),
        );
        assert_eq!(c, NEEDS_OK);
        assert_eq!(f.queue.for_home(&f.paths.root).len(), 1);
        assert_eq!(f.queue.get(&f.paths.root, &f.id).unwrap().items.len(), 2);
        assert!(state(&f).0.is_empty());
        // A stale code, an item outside the list and --keyboard are not queued at all.
        f.queue.clear(&f.paths.root, &f.id);
        for bad in [
            format!("grant {} hub.notify --listed {}", f.id, "0".repeat(16)),
            format!("grant {} net --listed {good}", f.id),
            format!("grant {} hub.notify --keyboard --listed {good}", f.id),
        ] {
            let (c, t) = ask(&f.pipe, &bad);
            assert_ne!(c, DONE, "{bad}: {t}");
            assert!(f.queue.for_home(&f.paths.root).is_empty(), "{bad}");
        }
        assert!(state(&f).0.is_empty());
    }

    #[test]
    fn deny_and_revoke_still_apply_directly_and_only_narrow() {
        let f = fx("narrow", &ping());
        grant_all(&f);
        let (c, t) = ask(&f.pipe, &format!("revoke {} ui.window", f.id));
        assert_eq!(c, DONE, "{t}");
        assert_eq!(state(&f).0, ["hub.notify"]);
        let user = fx_user("narrow2", &ping());
        let good = code(&user);
        let (c, t) = ask(
            &user.pipe,
            &format!("deny {} ui.window --listed {good}", user.id),
        );
        assert_eq!(c, DONE, "{t}");
        assert!(state(&user).0.is_empty());
        assert!(user.queue.for_home(&user.paths.root).is_empty());
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
        let (c, t) = ask(
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

    #[test]
    fn a_silent_client_does_not_stall_the_pipe() {
        let f = fx("silent", &ping());
        // Connects and says nothing, and stays connected.
        let silent = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&f.pipe)
            .unwrap();
        // The server makes its next pipe instance right after it accepts.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let t = std::time::Instant::now();
        let (c, text) = ask(&f.pipe, "frobnicate x");
        assert_eq!(c, USAGE, "{text}");
        assert!(
            t.elapsed() < std::time::Duration::from_secs(8),
            "the pipe was held by a client that sent nothing"
        );
        drop(silent);
    }

    /// A server that is not `hideout.exe` (here: this test binary under another expected name)
    /// that records every byte it is sent.
    fn squatter(name: &str) -> (std::thread::JoinHandle<Vec<u8>>, String) {
        use std::io::Read;
        let pipe = format!(
            r"\\.\pipe\hideout-control-squat-{name}-{}",
            std::process::id()
        );
        let first = foxglove_plugin::channel::ServerPipe::create(&pipe, true).expect("squat");
        let h = std::thread::spawn(move || {
            let mut got = Vec::new();
            // Waits for a connection; the refused client closes it without a byte.
            if let Ok(mut c) = first.accept() {
                let _ = c.read_to_end(&mut got);
            }
            got
        });
        (h, pipe)
    }

    #[test]
    fn a_squatting_server_is_refused_and_gets_nothing() {
        let (h, pipe) = squatter("refuse");
        // The squatter runs as the right user in the right folder, but is not hideout.exe:
        // the product's own identity (`Expect::HIDEOUT`) refuses it.
        let (c, t) = request(&pipe, "grant example.x --all-listed --listed 1234");
        assert_eq!(c, REFUSED, "{t}");
        assert!(t.starts_with("hideout.pipe.foreign"), "{t}");
        assert!(t.contains("nothing was sent"), "{t}");
        // File name only in the text: no folder, no pid.
        assert!(
            !t.contains('\\') && !t.contains(&std::process::id().to_string()),
            "{t}"
        );
        // The connection was closed unread: the squatter saw no request bytes.
        assert!(h.join().unwrap().is_empty(), "the squatter was sent bytes");
    }

    #[test]
    fn the_cli_front_end_refuses_a_squatter_too() {
        let f = fx("squatcli", &ping());
        // The real control server runs in this process (not hideout.exe): the product client
        // must refuse it before sending, so nothing is granted.
        let os = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let good = code(&f);
        let rc = client(
            &f.paths,
            os(&[
                "--modules",
                "grant",
                &f.id,
                "--all-listed",
                "--listed",
                &good,
            ])
            .into_iter(),
        );
        assert_eq!(rc, Some(REFUSED));
        assert!(
            state(&f).0.is_empty(),
            "a refused server still got the grant"
        );
        // The same server with the matching identity is used as before.
        assert_eq!(
            request_to(&f.pipe, &format!("revoke {} --all", f.id), own_exe()).0,
            DONE
        );
    }

    #[test]
    fn no_server_is_still_exit_5_and_the_expected_identity_is_served() {
        assert_eq!(
            request(r"\\.\pipe\hideout-control-squat-none-1184", "x").0,
            NO_HUB
        );
        let f = fx("ident", &ping());
        let (c, t) = ask(&f.pipe, "frobnicate x");
        assert_eq!(c, USAGE, "{t}");
    }
}
