// SPDX-License-Identifier: MIT
//! Which modules start in an AppContainer, and with which capabilities (contract §7.7: "an
//! AppContainer when the permissions allow it"). Pure: the module's GRANTED permissions and its
//! transport in, a plan out; [`crate::sandbox`] carries it out on Windows.
//!
//! What the hub brokers (facts, lights, the network calls of `net.connect` / `net.listen`, GPU,
//! secrets by reference, app watches) needs nothing from Windows inside the container. A
//! permission whose work the module does itself through a device maps to a capability
//! ([`CAPABILITIES`]). With containment on, a module runs outside only when the user already
//! granted it what a container takes away, and the permission dialog said so: `net` ("use any
//! network, without the sandbox") or `files` ("read and write your files"). Anything else
//! that can't be contained (a stdio module, direct channels, Windows' credential store) fails
//! to start: it never runs without a sandbox while nobody was told (#1089 review).

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
    /// Doesn't start: it can't be contained and nothing the user approved says it may run
    /// without the sandbox. The reason, for the log (never drawn).
    Refused(String),
}

/// The plan for a module with `granted` permissions (ungranted ones count for nothing: a
/// declared `net` the user never approved doesn't take the module out) and its transport.
pub fn plan(granted: &[Capability], stdio: bool) -> Plan {
    if stdio {
        return Plan::Refused("a stdio module can't run in an AppContainer".into());
    }
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
            Capability::Kept(Permission::Secrets) => {
                return Plan::Refused(
                    "it uses Windows' credential store itself (secrets), which a container blocks"
                        .into(),
                )
            }
            Capability::ChannelOpen { .. } | Capability::ChannelOffer(_) => {
                return Plan::Refused(
                    "its direct channels' pipes don't admit an AppContainer yet".into(),
                )
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
pub fn plan_for(granted: &[String], stdio: bool) -> Plan {
    match granted
        .iter()
        .map(|g| Capability::parse(g))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(caps) => plan(&caps, stdio),
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
        assert_eq!(plan(&p, false), Plan::Contained(vec![]));
        assert_eq!(plan(&[], false), Plan::Contained(vec![]));
    }

    #[test]
    fn a_hid_device_gets_its_capability() {
        let p = caps(&["device.hid:keyboard", "device.hid:consumer"]);
        assert_eq!(
            plan(&p, false),
            Plan::Contained(vec!["humaninterfacedevice"])
        );
    }

    #[test]
    fn granted_net_or_files_run_outside() {
        for c in ["net", "files"] {
            assert!(
                matches!(plan(&caps(&["lighting.write", c]), false), Plan::Outside(_)),
                "{c}"
            );
        }
    }

    #[test]
    fn stdio_secrets_and_channels_are_refused() {
        assert!(matches!(plan(&[], true), Plan::Refused(_)));
        // Even with net granted: a stdio module never runs uncontained without a word.
        assert!(matches!(plan(&caps(&["net"]), true), Plan::Refused(_)));
        for c in [
            "secrets",
            "channel.open:foxglove.example-link/art",
            "channel.offer:previews",
        ] {
            assert!(
                matches!(plan(&caps(&["net", c]), false), Plan::Refused(_)),
                "{c}"
            );
        }
    }

    #[test]
    fn only_grants_count() {
        // Declared but not granted: not in the list, so contained.
        assert_eq!(
            plan_for(&["lighting.write".into()], false),
            Plan::Contained(vec![])
        );
        assert!(matches!(
            plan_for(&["no such permission".into()], false),
            Plan::Refused(_)
        ));
    }
}
