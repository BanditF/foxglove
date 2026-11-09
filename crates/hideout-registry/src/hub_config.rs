// SPDX-License-Identifier: MIT
//! `hideout.toml` (spec §7.3), the hub's own state: `[modules.<id>] enabled`, `[[sources]]` and
//! `[[trust.anchors]]` (§13.4, §13.6). Tables this crate doesn't use are kept as they are.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use hideout_contract::config::Config;
use hideout_contract::{Error, ErrorId};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HubConfig {
    /// The whole file, so a write keeps every table.
    table: toml::Table,
    pub config: Config,
}

impl HubConfig {
    pub fn parse(text: &str) -> Result<HubConfig, Error> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| Error::new(ErrorId::ManifestSyntax, e.to_string()))?;
        let config = Config::parse(text)?;
        Ok(HubConfig { table, config })
    }

    /// Reads `path`; a missing file is an empty config.
    pub fn load(path: &Path) -> io::Result<Result<HubConfig, Error>> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(HubConfig::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Ok(HubConfig::default())),
            Err(e) => Err(e),
        }
    }

    /// Checks sources and anchors against the root's family scopes (the `config.*` errors and
    /// `trust.scope-overlap`).
    pub fn validate(&self, root_scopes: &BTreeMap<String, Vec<String>>) -> Result<(), Error> {
        self.config.validate(root_scopes)
    }

    pub fn enabled(&self, id: &str) -> bool {
        self.table
            .get("modules")
            .and_then(|m| m.get(id))
            .and_then(|m| m.get("enabled"))
            .and_then(|e| e.as_bool())
            .unwrap_or(false)
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) {
        let modules = self
            .table
            .entry("modules")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(modules) = modules {
            let m = modules
                .entry(id)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(m) = m {
                m.insert("enabled".into(), toml::Value::Boolean(on));
            }
        }
    }

    /// Drops a removed module's table.
    pub fn forget(&mut self, id: &str) {
        if let Some(toml::Value::Table(modules)) = self.table.get_mut("modules") {
            modules.remove(id);
        }
    }

    /// Writes atomically. Only the hub writes this file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(&self.table).map_err(io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}
