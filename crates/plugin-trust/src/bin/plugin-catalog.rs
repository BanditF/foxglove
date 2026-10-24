//! Edits a plugin catalog before it's signed (#190). Used by `scripts/publish-plugins.ps1`.
//!
//! ```text
//! plugin-catalog new <catalog.json> --epoch N
//! plugin-catalog add <catalog.json> --dir <unpacked plugin> --zip <archive> --url <https://...> [--channel stable|beta]
//! plugin-catalog yank <catalog.json> <id> [version...] [--reason TEXT]
//! plugin-catalog revoke <catalog.json> <id> [version...] [--reason TEXT]
//! plugin-catalog bump <catalog.json> [--days 30]
//! ```

use std::path::{Path, PathBuf};

use foxglove_plugin_trust::{publish, Action, Catalog};

fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

/// Arguments that aren't flags or flag values.
fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            skip = true;
        } else {
            out.push(a.clone());
        }
    }
    out
}

fn load(path: &Path) -> Result<Catalog, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())
}

fn save(path: &Path, c: &Catalog) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn run(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let (Some(cmd), Some(file)) = (pos.first(), pos.get(1)) else {
        return Err("usage: plugin-catalog new|add|yank|revoke|bump <catalog.json> ...".into());
    };
    let path = PathBuf::from(file);
    let num = |name: &str, default: u64| flag(args, name).map_or(Ok(default), |v| v.parse().map_err(|_| format!("{name} must be a number")));
    match cmd.as_str() {
        "new" => {
            if path.exists() {
                return Err(format!("{file} already exists"));
            }
            save(&path, &publish::new_catalog(num("--epoch", 0)?))
        }
        "add" => {
            let need = |name: &str| flag(args, name).ok_or_else(|| format!("add needs {name}"));
            let entry = publish::entry_from(
                Path::new(&need("--dir")?),
                Path::new(&need("--zip")?),
                &need("--url")?,
                &flag(args, "--channel").unwrap_or_else(|| "stable".into()),
            )?;
            let mut c = load(&path)?;
            println!("added {} {} ({})", entry.id, entry.version, entry.channel);
            publish::add(&mut c, entry);
            save(&path, &c)
        }
        "yank" | "revoke" => {
            let id = pos.get(2).ok_or("which plugin id?")?;
            let action = if cmd == "yank" { Action::Yank } else { Action::Revoke };
            let mut c = load(&path)?;
            publish::withdraw(&mut c, id, pos[3..].to_vec(), action, &flag(args, "--reason").unwrap_or_default());
            save(&path, &c)
        }
        "bump" => {
            let mut c = load(&path)?;
            publish::bump(&mut c, num("--days", 30)?);
            println!("sequence {}, expires {}", c.sequence, c.expires);
            save(&path, &c)
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("plugin-catalog: {e}");
        std::process::exit(1);
    }
}
