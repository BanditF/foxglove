// SPDX-License-Identifier: MIT
//! Which modules start in an AppContainer, and with which capabilities (contract §7.7: "an
//! AppContainer when the permissions allow it"). Pure: the module's GRANTED permissions in, a
//! plan out; [`crate::sandbox`] carries it out on Windows.
//!
//! What the hub brokers (facts, lights, the network calls of `net.connect` / `net.listen`, GPU,
//! secrets, app watches, audio) needs nothing from Windows inside the container. A permission
//! whose work the module does itself through a device maps to a capability ([`CAPABILITIES`]).
//! With containment on, a module runs outside only when the user already granted it what a
//! container takes away, and the permission dialog said so: `net` ("use any network, without
//! the sandbox") or `files` ("read and write your files").
//!
//! Contained since #1092, with the reasons in §7.7:
//! - **stdio modules**: their stdin and stdout are anonymous pipes the hub makes and hands over
//!   in an explicit handle list ([`crate::sandbox::Container::spawn_stdio`]). Nothing else to
//!   admit: they never open the router pipe.
//! - **`secrets`**: the hub keeps them in Credential Manager and answers `secrets.get`; the
//!   module never touches the store (a module that tried would be refused inside, which is
//!   right).
//! - **channels**: a contained provider makes its pipe in its own namespace, and a contained
//!   consumer is let into the provider's pipe by its SID (§8.4).

use std::collections::BTreeSet;

use hideout_contract::capability::Capability;
use hideout_contract::permission::Permission;

/// Permissions the module carries out itself through Windows, and the AppContainer capability
/// each needs (a name for `DeriveCapabilitySidsFromName`). Data: a new device class is a row.
pub const CAPABILITIES: &[(&str, &str)] = &[("device.hid", "humaninterfacedevice")];

/// What happens to a module when containment is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// In its AppContainer, with these capability names.
    Contained(Vec<&'static str>),
    /// Outside, as the user granted (`net` or `files`): the reason, for the log.
    Outside(String),
    /// Doesn't start: the plan can't be made (a grant that doesn't parse). The reason, for the
    /// log (never drawn).
    Refused(String),
}

/// The plan for a module with `granted` permissions (ungranted ones count for nothing: a
/// declared `net` the user never approved doesn't take the module out). Its transport doesn't
/// matter (#1092).
pub fn plan(granted: &[Capability]) -> Plan {
    let mut caps = BTreeSet::new();
    let mut outside = None;
    for p in granted {
        match p {
            Capability::Kept(Permission::Net) => {
                outside.get_or_insert("granted any network (net)");
            }
            Capability::Kept(Permission::Files) => {
                outside.get_or_insert("granted your files (files)");
            }
            Capability::DeviceHid(_) => caps.extend(
                CAPABILITIES
                    .iter()
                    .filter(|(perm, _)| *perm == "device.hid")
                    .map(|(_, cap)| *cap),
            ),
            _ => {}
        }
    }
    match outside {
        Some(why) => Plan::Outside(why.into()),
        None => Plan::Contained(caps.into_iter().collect()),
    }
}

/// [`plan`] for the grants as `inventory.json` stores them (canonical strings). One that
/// doesn't parse is refused: the plan never guesses.
pub fn plan_for(granted: &[String]) -> Plan {
    match granted
        .iter()
        .map(|g| Capability::parse(g))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(caps) => plan(&caps),
        Err(e) => Plan::Refused(format!("a grant doesn't parse: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(list: &[&str]) -> Vec<Capability> {
        list.iter().map(|c| Capability::parse(c).unwrap()).collect()
    }

    #[test]
    fn brokered_permissions_are_contained_without_capabilities() {
        let p = caps(&[
            "facts.read:clock.*",
            "lighting.write",
            "net.connect:example.com:443",
            "key.events",
        ]);
        assert_eq!(plan(&p), Plan::Contained(vec![]));
        assert_eq!(plan(&[]), Plan::Contained(vec![]));
    }

    #[test]
    fn a_hid_device_gets_its_capability() {
        let p = caps(&["device.hid:keyboard", "device.hid:consumer"]);
        assert_eq!(plan(&p), Plan::Contained(vec!["humaninterfacedevice"]));
    }

    #[test]
    fn granted_net_or_files_run_outside() {
        for c in ["net", "files"] {
            assert!(
                matches!(plan(&caps(&["lighting.write", c])), Plan::Outside(_)),
                "{c}"
            );
        }
    }

    /// #1092: what #1089 refused is contained now: secrets are the hub's, channels' pipes admit
    /// a container.
    #[test]
    fn secrets_and_channels_are_contained() {
        for c in [
            "secrets",
            "channel.open:foxglove.example-link/art",
            "channel.offer:previews",
        ] {
            assert_eq!(plan(&caps(&[c])), Plan::Contained(vec![]), "{c}");
        }
        // A granted `net` still takes the module out, whatever else it has.
        assert!(matches!(
            plan(&caps(&["net", "secrets", "channel.offer:x"])),
            Plan::Outside(_)
        ));
    }

    #[test]
    fn only_grants_count() {
        // Declared but not granted: not in the list, so contained.
        assert_eq!(
            plan_for(&["lighting.write".into()]),
            Plan::Contained(vec![])
        );
        assert!(matches!(
            plan_for(&["no such permission".into()]),
            Plan::Refused(_)
        ));
    }

    /// Every bridged plugin in this repository (each `crates/*/module.toml` with `transport =
    /// "stdio"`), every permission of its manifest granted (#1092): it is contained, or it
    /// runs outside only for a granted `net` or `files`. None is refused. Read from the files,
    /// so the hub names no plugin.
    #[test]
    fn every_bridged_plugin_in_the_repository_is_contained_or_granted_out() {
        use hideout_contract::capability::Capability as C;
        use hideout_contract::module::{Context, Module, Transport};
        let ctx = Context {
            private_ids: Vec::new(),
        };
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let (mut contained, mut outside) = (0, 0);
        for dir in std::fs::read_dir(crates).unwrap().flatten() {
            let Ok(text) = std::fs::read_to_string(dir.path().join("module.toml")) else {
                continue;
            };
            let m = Module::parse(&text, &ctx).unwrap();
            if m.transport != Transport::Stdio {
                continue;
            }
            let kept = |p: Permission| m.permissions.contains(&C::Kept(p));
            match plan(&m.permissions) {
                Plan::Contained(_) => contained += 1,
                Plan::Outside(_) if kept(Permission::Net) || kept(Permission::Files) => {
                    outside += 1
                }
                p => panic!("{}: {p:?}", m.id),
            }
        }
        assert!(contained >= 4, "{contained} contained, {outside} outside");
    }
}
