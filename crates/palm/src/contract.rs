//! Reusable checks for backend tests. Run on a disposable device: writes and
//! unconfirmed behavior attempts are made. Confirmed behaviors are never invoked.
use crate::*;

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(PalmError::Backend(format!("contract: {message}")))
    }
}

/// Checks advertised capabilities and exercises supported keymap writes using
/// an existing binding. Use `check_binding` too with a distinct valid binding
/// supplied by the backend's tests to catch ignored writes.
pub fn check_device(device: &dyn Device) -> Result<()> {
    let caps = device.caps();
    for (cap, present) in [
        (Caps::KEYMAP, device.keymap().is_some()),
        (Caps::LAYERS, device.layers().is_some()),
        (Caps::BEHAVIORS, device.behaviors().is_some()),
        (Caps::LIGHTING, device.lighting().is_some()),
        (Caps::COLOR_IN, device.color_source().is_some()),
        (Caps::SCREENS, device.screens().is_some()),
        (Caps::POWER, device.power().is_some()),
        (Caps::SETTINGS, device.settings().is_some()),
    ] {
        require(
            caps.contains(cap) == present,
            "capability/accessor mismatch",
        )?;
    }
    require(
        caps.contains(Caps::KEYMAP_WRITE)
            == device
                .keymap()
                .is_some_and(|k| k.write_support() != WriteSupport::None),
        "keymap write flag mismatch",
    )?;
    if let Some(keymap) = device.keymap() {
        let layout = keymap.layout()?;
        let layers = keymap.layers()?;
        if let (Some(key), Some(layer)) = (layout.keys.first(), layers.first()) {
            let original = keymap.binding(layer.id, key.row, key.col)?;
            match keymap.write_support() {
                WriteSupport::Immediate => {
                    check_binding(keymap, layer.id, key.row, key.col, original)?
                }
                WriteSupport::None => {
                    check_unsupported(keymap.set_binding(layer.id, key.row, key.col, original))?;
                    check_unsupported(keymap.save())?;
                    check_unsupported(keymap.discard())?;
                }
                WriteSupport::Session => {}
            }
        }
    }
    if let Some(behaviors) = device.behaviors() {
        check_behaviors(behaviors)?;
    }
    Ok(())
}

pub fn check_behaviors(behaviors: &dyn Behaviors) -> Result<()> {
    for behavior in behaviors.list()? {
        if behavior.risk == Risk::Confirm {
            require(
                matches!(
                    behaviors.invoke(behavior.id, behavior.param1, behavior.param2, false),
                    Err(PalmError::Denied(_))
                ),
                "confirmation was not enforced",
            )?;
        }
    }
    Ok(())
}

/// Writes a backend-valid binding, verifies it, and restores the original.
pub fn check_binding(
    keymap: &dyn Keymap,
    layer: u32,
    row: u16,
    col: u16,
    binding: Binding,
) -> Result<()> {
    require(
        keymap.write_support() == WriteSupport::Immediate,
        "expected Immediate keymap",
    )?;
    let original = keymap.binding(layer, row, col)?;
    keymap.set_binding(layer, row, col, binding.clone())?;
    let observed = keymap.binding(layer, row, col);
    let restored = keymap.set_binding(layer, row, col, original);
    let observed = observed?;
    restored?;
    require(observed == binding, "keymap write did not read back")
}

/// Backends can pass each operation they intentionally leave unsupported.
pub fn check_unsupported<T>(result: Result<T>) -> Result<()> {
    require(
        matches!(result, Err(PalmError::Unsupported)),
        "expected Unsupported",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockBackend;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn mock_passes_contract_and_capabilities_work() {
        let backend = MockBackend::default();
        let info = backend.discover().unwrap().remove(0);
        let bus = EventBus::default();
        let device = backend.open(&info, bus.sink(info.id.clone())).unwrap();
        check_device(device.as_ref()).unwrap();
        check_binding(
            device.keymap().unwrap(),
            0,
            0,
            0,
            Binding {
                kind: "key".into(),
                params: vec![5],
                label: Some("B".into()),
            },
        )
        .unwrap();
        let layers = device.layers().unwrap();
        layers.set(&[0, 1]).unwrap();
        layers.toggle(1).unwrap();
        assert_eq!(layers.active().unwrap(), vec![0]);
        let lighting = device.lighting().unwrap();
        let effect = Effect {
            id: "rainbow".into(),
            params: vec![3],
        };
        lighting.set_effect(effect.clone()).unwrap();
        assert_eq!(lighting.state().unwrap().effect, effect);
        lighting.set_frame(&[Rgb { r: 1, g: 2, b: 3 }]).unwrap();
        lighting.release().unwrap();
        let settings = device.settings().unwrap();
        settings.set("sync", Value::Bool(true)).unwrap();
        assert_eq!(settings.get("sync").unwrap(), Some(Value::Bool(true)));
        assert_eq!(device.power().unwrap().batteries().unwrap()[0].1, 100);
        let screens = device.screens().unwrap();
        screens.send_panel(&[255], 8, 1).unwrap();
        let mut progress = Vec::new();
        screens
            .push_art(
                &[1; 128],
                &mut |n, total| progress.push((n, total)),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(progress.last(), Some(&(128, 128)));
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            screens.push_art(
                &[2; 128],
                &mut |n, _| {
                    if n >= 64 {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
                &cancel
            ),
            Err(PalmError::Denied(_))
        ));
        assert_eq!(screens.art_status().unwrap().bytes, 128);
        screens.select_art(Some(2)).unwrap();
        assert_eq!(screens.art_status().unwrap().selected, Some(2));
        let behaviors = device.behaviors().unwrap();
        assert!(matches!(
            behaviors.invoke(0, 1, 2, false),
            Err(PalmError::Denied(_))
        ));
        assert_eq!(behaviors.invoke(0, 9, 9, true), Err(PalmError::Unsupported));
        behaviors.invoke(0, 1, 2, true).unwrap();
    }

    #[test]
    fn rediscovery_reconnect_and_backend_disconnect() {
        let backend = MockBackend::default();
        let present = backend.present.clone();
        let mut registry = Registry::default();
        registry.add_backend(Box::new(backend)).unwrap();
        let events = registry.events().subscribe(false);
        let id = registry.discover().devices[0].info.id.clone();
        registry.open(&id).unwrap();
        assert_eq!(registry.discover().devices[0].state, DeviceState::Open);
        assert!(matches!(registry.open(&id), Err(PalmError::Busy(_))));
        registry
            .events()
            .sink(id.clone())
            .emit(DeviceEvent::Disconnected {
                reason: "link lost".into(),
            });
        assert_eq!(registry.list()[0].state, DeviceState::Disconnected);
        assert!(registry.device(&id).is_none());
        registry.open(&id).unwrap();
        present.store(false, Ordering::Relaxed);
        assert_eq!(
            registry.discover().devices[0].state,
            DeviceState::Disconnected
        );
        assert!(matches!(registry.open(&id), Err(PalmError::Disconnected)));
        present.store(true, Ordering::Relaxed);
        assert_eq!(registry.discover().devices[0].info.id, id);
        registry.open(&id).unwrap();
        registry.close(&id).unwrap();
        assert_eq!(registry.list()[0].state, DeviceState::Available);
        assert_eq!(events.try_iter().count(), 6);
    }

    struct Minimal(DeviceInfo);

    impl Device for Minimal {
        fn info(&self) -> &DeviceInfo {
            &self.0
        }

        fn caps(&self) -> Caps {
            Caps::default()
        }
    }

    struct ReadOnly;

    impl Keymap for ReadOnly {
        fn layout(&self) -> Result<Layout> {
            Err(PalmError::Unsupported)
        }

        fn layers(&self) -> Result<Vec<LayerInfo>> {
            Err(PalmError::Unsupported)
        }

        fn binding(&self, _: u32, _: u16, _: u16) -> Result<Binding> {
            Err(PalmError::Unsupported)
        }
    }

    impl Layers for ReadOnly {
        fn active(&self) -> Result<Vec<u8>> {
            Ok(vec![])
        }
    }

    impl Screens for ReadOnly {}

    #[test]
    fn absent_capabilities_and_missing_operations() {
        let info = MockBackend::default().discover().unwrap().remove(0);
        check_device(&Minimal(info)).unwrap();
        let empty = Binding {
            kind: "none".into(),
            params: vec![],
            label: None,
        };
        check_unsupported(ReadOnly.set_binding(0, 0, 0, empty)).unwrap();
        check_unsupported(ReadOnly.save()).unwrap();
        check_unsupported(ReadOnly.discard()).unwrap();
        check_unsupported(ReadOnly.set(&[])).unwrap();
        check_unsupported(ReadOnly.toggle(0)).unwrap();
        check_unsupported(ReadOnly.send_panel(&[], 0, 0)).unwrap();
        check_unsupported(ReadOnly.art_status()).unwrap();
        check_unsupported(ReadOnly.select_art(None)).unwrap();
    }

    #[test]
    fn identity_prefers_serial_and_has_no_namespace_dots() {
        let backend = BackendId("mock.backend".into());
        let a = DeviceId::new(backend.clone(), 1, 2, Some("serial.1"), "path-a");
        let b = DeviceId::new(backend.clone(), 1, 2, Some("serial.1"), "path-b");
        assert_eq!(a, b);
        assert!(!a.to_string().contains('.'));
        assert_ne!(a, DeviceId::new(backend, 1, 2, None, "serial.1"));
    }
}
