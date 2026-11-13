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
        log::warn!("permissions: control pipe: refused a client inside a module job or not hideout");
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
            let mut next = Some(first);
            loop {
                let pipe = match next.take() {
                    Some(p) => p,
                    None => match ServerPipe::create(&name, false) {
                        Ok(p) => p,
                        Err(e) => {
                            log::error!("control: {name}: {e}");
                            return;
                        }
                    },
                };
                let Ok(mut conn) = pipe.accept() else {
                    continue;
                };
                let ok = conn.client_pid().is_ok_and(|p| caller_ok(&sup, p));
                let mut line = String::new();
                let _ = BufReader::new(conn.clone())
                    .take(4096)
                    .read_line(&mut line);
                let (code, text) = handle(&home, &hub, ok, &line);
                let _ = conn.write_all(format!("{code}\n{text}").as_bytes());
            }
        })?;
    Ok(())
}

/// The client: sends one request line, returns (exit code, text). No hub = exit 5.
pub fn request(pipe: &str, line: &str) -> (i32, String) {
    use std::io::{Read, Write};
    let Ok(mut c) = std::fs::OpenOptions::new().read(true).write(true).open(pipe) else {
        return (NO_HUB, "hideout: the hub isn't running; nothing changed\n".into());
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
