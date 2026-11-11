// SPDX-License-Identifier: MIT
//! Which modules start in an AppContainer, and with which capabilities (contract §7.7: "an
//! AppContainer when the permissions allow it"). Pure: the manifest's permissions in, a plan
//! out; [`crate::sandbox`] carries it out on Windows.
//!
//! The rule follows Foxglove's plugin host (#215): what the hub brokers (facts, lights, the
//! network calls of `net.connect` / `net.listen`, GPU, secrets by reference, app watches) needs
//! nothing from Windows inside the container. A permission whose work the module does itself
//! through Windows either maps to a capability the container gets ([`CAPABILITIES`]) or keeps
//! the module outside the container.

use std::collections::BTreeSet;

use hideout_contract::capability::Capability;
use hideout_contract::permission::Permission;

/// Permissions the module carries out itself through Windows, and the AppContainer capability
/// each needs (a name for `DeriveCapabilitySidsFromName`). Data: a new device class is a row.
pub const CAPABILITIES: &[(&str, &str)] = &[("device.hid", "humaninterfacedevice")];

/// The capability names for a module with `permissions`, or why it runs outside the container
/// (plain words for the log; never drawn).
pub fn plan(permissions: &[Capability]) -> Result<Vec<&'static str>, String> {
    let mut caps = BTreeSet::new();
    for p in permissions {
        let outside = match p {
            Capability::Kept(Permission::Net) => "it asks for any network (net)",
            Capability::Kept(Permission::Files) => "it reads and writes your files (files)",
            Capability::Kept(Permission::Secrets) => {
                "it uses Windows' credential store itself (secrets)"
            }
            Capability::ChannelOpen { .. } | Capability::ChannelOffer(_) => {
                "its direct channels' pipes don't admit an AppContainer yet"
            }
            Capability::DeviceHid(_) => {
                caps.extend(
                    CAPABILITIES
                        .iter()
                        .filter(|(perm, _)| *perm == "device.hid")
                        .map(|(_, cap)| *cap),
                );
                continue;
            }
            _ => continue,
        };
        return Err(outside.to_string());
    }
    Ok(caps.into_iter().collect())
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
        assert_eq!(plan(&p), Ok(vec![]));
        assert_eq!(plan(&[]), Ok(vec![]));
    }

    #[test]
    fn a_hid_device_gets_its_capability() {
        let p = caps(&["device.hid:keyboard", "device.hid:consumer"]);
        assert_eq!(plan(&p), Ok(vec!["humaninterfacedevice"]));
    }

    #[test]
    fn net_files_secrets_and_channels_run_outside() {
        for c in [
            "net",
            "files",
            "secrets",
            "channel.open:foxglove.example-link/art",
            "channel.offer:previews",
        ] {
            assert!(plan(&caps(&["lighting.write", c])).is_err(), "{c}");
        }
    }
}
