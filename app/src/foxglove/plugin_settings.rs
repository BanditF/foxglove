//! Plugin settings (#69): a plugin declares options in `plugin.toml` (`[[settings]]`), the user
//! sets them in the Plugins tab, and they live in the store's `[plugins.settings]`. The plugin
//! reads them with `settings.get` and is told of a change with `settings.changed`.

use std::collections::BTreeMap;

use foxglove_plugin_api::manifest::{Manifest, SettingDecl};
use serde_json::{Map, Value};

/// Plugin id to the values the user changed.
type Stored = BTreeMap<String, Map<String, Value>>;

pub fn stored() -> Stored {
    super::store::try_load("plugins.settings").unwrap_or_default()
}

/// Every declared setting: the user's value if it still fits the declaration, else the default.
pub fn values_for(manifest: &Manifest, stored: Option<&Map<String, Value>>) -> Map<String, Value> {
    manifest
        .settings
        .iter()
        .map(|d| {
            let v = stored
                .and_then(|s| s.get(&d.name))
                .and_then(|v| d.accept(v))
                .unwrap_or_else(|| d.default.clone());
            (d.name.clone(), v)
        })
        .collect()
}

/// A plugin's settings, by id (the host's settings source).
pub fn values(plugin: &str) -> Map<String, Value> {
    let Some(p) = super::plugin_install::installed()
        .into_iter()
        .find(|p| p.manifest.id == plugin)
    else {
        return Map::new();
    };
    values_for(&p.manifest, stored().get(plugin))
}

/// Saves one setting and tells the plugin.
pub fn set(plugin: &str, decl: &SettingDecl, value: Value) -> Result<(), String> {
    let value = decl
        .accept(&value)
        .ok_or("that isn't a value this setting takes")?;
    let mut all = stored();
    all.entry(plugin.to_string())
        .or_default()
        .insert(decl.name.clone(), value);
    super::store::save("plugins.settings", &all).map_err(|e| e.to_string())?;
    foxglove_plugin_host::settings_changed(plugin);
    Ok(())
}

pub fn start() {
    foxglove_plugin_host::set_settings_source(values);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_until_changed_and_stale_values_are_dropped() {
        let toml = r#"
id = "org.example.timer"
name = "Timer"
version = "1.0.0"
api = "1.3"
run = ["timer.exe"]

[[settings]]
name = "minutes"
label = "Minutes"
kind = "int"
default = 25
min = 1
max = 120

[[settings]]
name = "sound"
label = "Play a sound"
kind = "bool"
default = false
"#;
        let m = Manifest::parse(toml, foxglove_plugin_api::API_VERSION).unwrap();
        let d = values_for(&m, None);
        assert_eq!(
            (d["minutes"].clone(), d["sound"].clone()),
            (Value::from(25), Value::from(false))
        );
        let mut s = Map::new();
        s.insert("minutes".into(), Value::from(500));
        s.insert("sound".into(), Value::from("yes"));
        s.insert("gone".into(), Value::from(1));
        let v = values_for(&m, Some(&s));
        assert_eq!(v["minutes"], Value::from(120), "clamped to its range");
        assert_eq!(
            v["sound"],
            Value::from(false),
            "the wrong type falls back to the default"
        );
        assert!(!v.contains_key("gone"), "only declared settings");
    }
}
