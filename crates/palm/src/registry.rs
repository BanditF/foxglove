use crate::*;
use std::collections::{HashMap, HashSet};

/// The current availability of a known device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Available,
    Open,
    Disconnected,
}

/// Discovery metadata and current registry state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    pub info: DeviceInfo,
    pub state: DeviceState,
}

/// Known devices and discovery failures from each backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoverReport {
    pub devices: Vec<DeviceEntry>,
    pub errors: Vec<(BackendId, PalmError)>,
}

struct Entry {
    info: DeviceInfo,
    present: bool,
    device: Option<Box<dyn Device>>,
}

/// One owner of each open device. Rediscovery retains known identities.
#[derive(Default)]
pub struct Registry {
    backends: Vec<Box<dyn Backend>>,
    entries: HashMap<DeviceId, Entry>,
    bus: EventBus,
}

impl Registry {
    pub fn new(bus: EventBus) -> Self {
        Self {
            bus,
            backends: Vec::new(),
            entries: HashMap::new(),
        }
    }

    pub fn events(&self) -> &EventBus {
        &self.bus
    }

    pub fn add_backend(&mut self, backend: Box<dyn Backend>) -> Result<()> {
        if self.backends.iter().any(|b| b.id() == backend.id()) {
            return Err(PalmError::Busy("backend already registered".into()));
        }
        self.backends.push(backend);
        Ok(())
    }

    /// Applies successful discoveries and preserves entries for failed backends.
    pub fn discover(&mut self) -> DiscoverReport {
        let mut errors = Vec::new();
        for backend in &self.backends {
            let backend_id = backend.id();
            let found = backend.discover().and_then(|found| {
                let mut seen = HashSet::new();
                for info in &found {
                    if info.backend != backend_id
                        || info.id.backend != backend_id
                        || !seen.insert(info.id.clone())
                    {
                        return Err(PalmError::Backend(
                            "invalid or duplicate device identity".into(),
                        ));
                    }
                }
                Ok(found)
            });
            let found = match found {
                Ok(found) => found,
                Err(error) => {
                    errors.push((backend_id, error));
                    continue;
                }
            };
            let seen: HashSet<_> = found.iter().map(|info| info.id.clone()).collect();
            for (id, entry) in &mut self.entries {
                if entry.info.backend == backend_id && !seen.contains(id) {
                    if entry.device.take().is_some() {
                        self.bus.sink(id.clone()).emit(DeviceEvent::Disconnected {
                            reason: "removed".into(),
                        });
                    }
                    entry.present = false;
                }
            }
            for info in found {
                let entry = self
                    .entries
                    .entry(info.id.clone())
                    .or_insert_with(|| Entry {
                        info: info.clone(),
                        present: true,
                        device: None,
                    });
                entry.info = info;
                entry.present = true;
            }
        }
        DiscoverReport {
            devices: self.list(),
            errors,
        }
    }

    fn state(&self, entry: &Entry) -> DeviceState {
        if entry.device.is_some() {
            if self
                .bus
                .facts()
                .get(&format!("device.{}.connected", entry.info.id))
                == Some(Value::Bool(false))
            {
                DeviceState::Disconnected
            } else {
                DeviceState::Open
            }
        } else if entry.present {
            DeviceState::Available
        } else {
            DeviceState::Disconnected
        }
    }

    pub fn list(&self) -> Vec<DeviceEntry> {
        let mut entries: Vec<_> = self
            .entries
            .values()
            .map(|entry| DeviceEntry {
                info: entry.info.clone(),
                state: self.state(entry),
            })
            .collect();
        entries.sort_by_key(|e| e.info.id.to_string());
        entries
    }

    pub fn open(&mut self, id: &DeviceId) -> Result<&dyn Device> {
        let entry = self.entries.get(id).ok_or(PalmError::Disconnected)?;
        if self.state(entry) == DeviceState::Open {
            return Err(PalmError::Busy("device already open".into()));
        }
        if !entry.present {
            return Err(PalmError::Disconnected);
        }
        let entry = self.entries.get_mut(id).unwrap();
        entry.device.take();
        let backend = self
            .backends
            .iter()
            .find(|b| b.id() == entry.info.backend)
            .ok_or(PalmError::Disconnected)?;
        let sink = self.bus.sink(id.clone());
        let device = backend.open(&entry.info, sink.clone())?;
        if device.info().id != *id {
            return Err(PalmError::Backend("opened device identity changed".into()));
        }
        entry.device = Some(device);
        sink.emit(DeviceEvent::Connected);
        Ok(entry.device.as_deref().unwrap())
    }

    pub fn device(&self, id: &DeviceId) -> Option<&dyn Device> {
        let entry = self.entries.get(id)?;
        if self.state(entry) != DeviceState::Open {
            return None;
        }
        entry.device.as_deref()
    }

    pub fn close(&mut self, id: &DeviceId) -> Result<()> {
        let entry = self.entries.get_mut(id).ok_or(PalmError::Disconnected)?;
        if entry.device.take().is_some() {
            self.bus.sink(id.clone()).emit(DeviceEvent::Disconnected {
                reason: "closed".into(),
            });
        }
        Ok(())
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        for (id, entry) in &mut self.entries {
            if entry.device.take().is_some() {
                self.bus.sink(id.clone()).emit(DeviceEvent::Disconnected {
                    reason: "registry dropped".into(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct TestBackend {
        id: BackendId,
        discovery: Arc<Mutex<Result<Vec<DeviceInfo>>>>,
    }

    impl Backend for TestBackend {
        fn id(&self) -> BackendId {
            self.id.clone()
        }

        fn discover(&self) -> Result<Vec<DeviceInfo>> {
            self.discovery
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        fn open(&self, info: &DeviceInfo, _: EventSink) -> Result<Box<dyn Device>> {
            Ok(Box::new(TestDevice(info.clone())))
        }
    }

    struct TestDevice(DeviceInfo);

    impl Device for TestDevice {
        fn info(&self) -> &DeviceInfo {
            &self.0
        }

        fn caps(&self) -> Caps {
            Caps::default()
        }
    }

    fn info(backend: &str) -> DeviceInfo {
        let backend = BackendId(backend.into());
        DeviceInfo {
            id: DeviceId::new(backend.clone(), 1, 2, None, "path"),
            backend,
            name: "Test device".into(),
            kind: DeviceKind::Other,
            firmware: None,
        }
    }

    fn register(registry: &mut Registry, info: &DeviceInfo) -> Arc<Mutex<Result<Vec<DeviceInfo>>>> {
        let discovery = Arc::new(Mutex::new(Ok(vec![info.clone()])));
        registry
            .add_backend(Box::new(TestBackend {
                id: info.backend.clone(),
                discovery: discovery.clone(),
            }))
            .unwrap();
        discovery
    }

    #[test]
    fn failing_backend_is_preserved_while_working_backend_updates() {
        // Exercise both orders so a failure cannot prevent a later backend's update.
        for failing_first in [false, true] {
            let mut registry = Registry::default();
            let failed = info("failed");
            let working = info("working");
            let (failure, success) = if failing_first {
                (
                    register(&mut registry, &failed),
                    register(&mut registry, &working),
                )
            } else {
                let success = register(&mut registry, &working);
                (register(&mut registry, &failed), success)
            };
            assert!(registry.discover().errors.is_empty());
            registry.open(&failed.id).unwrap();
            registry.open(&working.id).unwrap();
            let events = registry.events().subscribe(false);
            *failure.lock().unwrap_or_else(|e| e.into_inner()) = Err(PalmError::Timeout);
            *success.lock().unwrap_or_else(|e| e.into_inner()) = Ok(vec![]);

            let report = registry.discover();
            assert_eq!(
                report.errors,
                vec![(failed.backend.clone(), PalmError::Timeout)]
            );
            assert_eq!(report.devices.len(), 2);
            assert!(registry.device(&failed.id).is_some());
            assert!(registry.device(&working.id).is_none());
            assert_eq!(
                report
                    .devices
                    .iter()
                    .find(|e| e.info.id == working.id)
                    .unwrap()
                    .state,
                DeviceState::Disconnected
            );
            let events: Vec<_> = events.try_iter().collect();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].device, working.id);

            // A successful empty result after recovery must close the retained device.
            *failure.lock().unwrap_or_else(|e| e.into_inner()) = Ok(vec![]);
            let report = registry.discover();
            assert!(report.errors.is_empty());
            assert!(registry.device(&failed.id).is_none());
            assert!(report
                .devices
                .iter()
                .all(|e| e.state == DeviceState::Disconnected));

            *failure.lock().unwrap_or_else(|e| e.into_inner()) = Ok(vec![failed.clone()]);
            assert!(registry.discover().errors.is_empty());
            registry.open(&failed.id).unwrap();
        }
    }

    #[test]
    fn invalid_identities_fail_only_their_backend_without_partial_updates() {
        for invalid in 0..3 {
            let mut registry = Registry::default();
            let failed = info("failed");
            let working = info("working");
            let failure = register(&mut registry, &failed);
            let success = register(&mut registry, &working);
            registry.discover();
            registry.open(&failed.id).unwrap();
            let before = registry.list();
            let events = registry.events().subscribe(false);
            let mut changed = failed.clone();
            changed.name = "Must not apply".into();
            let mut bad = failed.clone();
            match invalid {
                0 => bad.backend = working.backend.clone(),
                1 => bad.id.backend = working.backend.clone(),
                _ => {}
            }
            *failure.lock().unwrap_or_else(|e| e.into_inner()) = Ok(vec![changed, bad]);
            let mut updated = working.clone();
            updated.name = "Updated".into();
            *success.lock().unwrap_or_else(|e| e.into_inner()) = Ok(vec![updated.clone()]);

            let report = registry.discover();
            assert_eq!(report.errors.len(), 1);
            assert_eq!(report.errors[0].0, failed.backend);
            assert!(matches!(report.errors[0].1, PalmError::Backend(_)));
            assert_eq!(
                report.devices.iter().find(|e| e.info.id == failed.id),
                before.iter().find(|e| e.info.id == failed.id)
            );
            assert!(registry.device(&failed.id).is_some());
            assert_eq!(
                report
                    .devices
                    .iter()
                    .find(|e| e.info.id == working.id)
                    .unwrap()
                    .info,
                updated
            );
            assert!(events.try_recv().is_err());
        }
    }
}
