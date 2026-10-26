//! %APPDATA%\hostpanel\config.toml

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

const DEFAULT: &str = r#"# hostpanel settings. Changes apply within a few seconds.

# Only use keyboards whose USB product name contains this (leave out to take the first one that
# answers).
# product = "Eyelash"

# Panel redraws per second (the media title scrolls at this rate).
fps = 4

# Show a hint on the panel while one of these programs is in front. Keys are exe names
# (case-insensitive), values are what the panel says.
[games]
# "valorant.exe" = "GAME: WASD"
# "factorio.exe" = "L2 = BUILD"
"#;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub product: Option<String>,
    pub fps: u32,
    pub games: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            product: None,
            fps: 4,
            games: HashMap::new(),
        }
    }
}

pub fn path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hostpanel")
        .join("config.toml")
}

/// Reads the config, writing the commented default first if there is none.
pub fn load() -> Config {
    let p = path();
    if !p.exists() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, DEFAULT);
    }
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let mut cfg: Config = toml::from_str(&text).unwrap_or_else(|e| {
        eprintln!("{}: {e}; using defaults", p.display());
        Config::default()
    });
    cfg.fps = cfg.fps.clamp(1, 10);
    cfg.games = cfg
        .games
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();
    cfg
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_parses() {
        let c: super::Config = toml::from_str(super::DEFAULT).unwrap();
        assert_eq!(c.fps, 4);
    }
}
