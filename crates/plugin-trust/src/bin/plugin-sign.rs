//! Makes the plugin keys and signs the catalog (#785). Built with `--features sign`; see
//! docs/plugin-catalog-launch.md.
//!
//! ```text
//! plugin-sign keygen <dir> <name> [--no-password]
//! plugin-sign root --key <root.key> --catalog-pub <catalog.pub> --out <dir>
//!                  [--pin <root.pub>] [--version 1] [--epoch 0] [--days 730]
//! plugin-sign catalog --key <catalog.key> --pin <root.pub> --root <dir> --source <catalog.json>
//!                     --out <dir> [--published <catalog.json>] [--days 30]
//! ```
//!
//! A key's password comes from `FOXGLOVE_SIGN_PASSWORD` when it's set (empty for a key without
//! one), otherwise from the terminal. Secret keys are only read from the file named; nothing
//! here prints one.

use std::path::{Path, PathBuf};

use foxglove_plugin_trust::{sign, Catalog, Root, Seen};

const PASSWORD_ENV: &str = "FOXGLOVE_SIGN_PASSWORD";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn need(args: &[String], name: &str) -> Result<String, String> {
    flag(args, name).ok_or_else(|| format!("needs {name}"))
}

fn num(args: &[String], name: &str, default: u64) -> Result<u64, String> {
    flag(args, name).map_or(Ok(default), |v| {
        v.parse().map_err(|_| format!("{name} must be a number"))
    })
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_text(path: &Path) -> Result<String, String> {
    String::from_utf8(read(path)?).map_err(|_| format!("{}: not UTF-8", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes a new file only: a key is never overwritten.
fn create(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn secret(path: &str) -> Result<minisign::SecretKey, String> {
    sign::load_secret(
        &read_text(Path::new(path))?,
        std::env::var(PASSWORD_ENV).ok(),
    )
}

fn keygen(args: &[String]) -> Result<(), String> {
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let (Some(dir), Some(name)) = (pos.first(), pos.get(1)) else {
        return Err("usage: plugin-sign keygen <dir> <name> [--no-password]".into());
    };
    let dir = PathBuf::from(dir);
    let password = if args.iter().any(|a| a == "--no-password") {
        Some(String::new())
    } else {
        std::env::var(PASSWORD_ENV).ok()
    };
    let k = sign::keygen(password)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    create(&dir.join(format!("{name}.key")), k.secret.as_bytes())?;
    create(&dir.join(format!("{name}.pub")), k.public.as_bytes())?;
    println!("{}", dir.join(format!("{name}.key")).display());
    println!("{}", dir.join(format!("{name}.pub")).display());
    println!("public key: {}", k.public_base64);
    Ok(())
}

fn root(args: &[String]) -> Result<(), String> {
    let sk = secret(&need(args, "--key")?)?;
    let catalog_key = sign::public_base64(&read_text(Path::new(&need(args, "--catalog-pub")?))?)?;
    let out = PathBuf::from(need(args, "--out")?);
    let (root, json, sig) = sign::sign_root(
        &sk,
        vec![catalog_key],
        num(args, "--version", 1)?,
        num(args, "--epoch", 0)?,
        num(args, "--days", 730)?,
    )?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    write(&out.join("root.json"), &json)?;
    write(&out.join("root.json.minisig"), sig.as_bytes())?;
    if let Some(pin) = flag(args, "--pin") {
        write(
            Path::new(&pin),
            format!("{}\n", sign::public_of(&sk)?).as_bytes(),
        )?;
        println!("pinned the root key in {pin}");
    }
    println!(
        "root.json version {}, epoch {}, expires {}",
        root.version, root.epoch, root.expires
    );
    Ok(())
}

fn catalog(args: &[String]) -> Result<(), String> {
    let pinned = sign::public_base64(&read_text(Path::new(&need(args, "--pin")?))?)?;
    let root_dir = PathBuf::from(need(args, "--root")?);
    let root_json = read(&root_dir.join("root.json"))?;
    let root_sig = read_text(&root_dir.join("root.json.minisig"))?;
    let now = foxglove_plugin_trust::now_utc();
    let root: Root =
        foxglove_plugin_trust::verify_root(&root_json, &root_sig, &pinned, &now, &Seen::default())
            .map_err(|e| format!("root.json: {e}"))?;
    let load = |path: &str| -> Result<Catalog, String> {
        serde_json::from_slice(&read(Path::new(path))?).map_err(|e| format!("{path}: {e}"))
    };
    let source = load(&need(args, "--source")?)?;
    let published = flag(args, "--published").map(|p| load(&p)).transpose()?;
    let next = sign::next_catalog(&source, &root, published.as_ref(), num(args, "--days", 30)?);
    let sk = secret(&need(args, "--key")?)?;
    let (json, sig) = sign::sign_catalog(&sk, &root, &next)?;
    let out = PathBuf::from(need(args, "--out")?);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    write(&out.join("root.json"), &root_json)?;
    write(&out.join("root.json.minisig"), root_sig.as_bytes())?;
    write(&out.join("catalog.json"), &json)?;
    write(&out.join("catalog.json.minisig"), sig.as_bytes())?;
    println!(
        "catalog sequence {}, epoch {}, {} plugin version(s), expires {}",
        next.sequence,
        next.epoch,
        next.plugins.len(),
        next.expires
    );
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(&args[1..]),
        Some("root") => root(&args[1..]),
        Some("catalog") => catalog(&args[1..]),
        _ => Err("usage: plugin-sign keygen|root|catalog ...".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("plugin-sign: {e}");
        std::process::exit(1);
    }
}
