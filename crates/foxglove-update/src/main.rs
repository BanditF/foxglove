use base64::{engine::general_purpose::STANDARD, Engine};
use foxglove_update::{hash, Manifest, Msi, Result};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair},
};
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let mut seed = [0;32];
            SystemRandom::new().fill(&mut seed).map_err(|_| "Random key generation failed")?;
            let key = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| "Invalid seed")?;
            println!("private: {}\npublic: {}", STANDARD.encode(seed), STANDARD.encode(key.public_key().as_ref()));
        }
        Some("sign") => {
            let path = args.get(1).ok_or("sign <latest.json>")?;
            let seed = STANDARD.decode(std::env::var("FOXGLOVE_UPDATE_KEY")?.trim())?;
            let key = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| "Expected 32-byte private key")?;
            std::fs::write(format!("{path}.sig"), STANDARD.encode(key.sign(&std::fs::read(path)?).as_ref()))?;
        }
        Some("manifest") => {
            let flag = |name: &str| args.windows(2).find(|p| p[0] == name).map(|p| p[1].clone()).ok_or_else(|| format!("Missing {name}"));
            let version = flag("--version")?;
            if foxglove_update::tag_version(&format!("app-v{version}")).is_none() { return Err("Invalid stable version".into()); }
            let (sha256, size) = hash(std::fs::File::open(flag("--msi")?)?)?;
            let manifest = Manifest { tag: format!("app-v{version}"), version, msi: Msi { name: "foxglove-x64.msi".into(), url: flag("--url")?, sha256, size }, notes: std::fs::read_to_string(flag("--notes-file")?)?, published: chrono::Utc::now().to_rfc3339() };
            std::fs::write("latest.json", serde_json::to_vec_pretty(&manifest)?)?;
        }
        _ => return Err("Usage: keygen | sign <latest.json> | manifest --version V --msi PATH --url URL --notes-file PATH".into()),
    }
    Ok(())
}
