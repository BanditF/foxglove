//! Thread-safe facts with ordered change notifications and no polling.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};

/// A small value shared by device settings and facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

/// A fact update, or removal when the value is absent.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub key: String,
    pub value: Option<Value>,
}

#[derive(Default)]
struct Inner {
    values: HashMap<String, Value>,
    subscribers: Vec<(String, Sender<Change>)>,
}

/// A shared fact store with change subscriptions.
#[derive(Clone, Default)]
pub struct Facts {
    inner: Arc<Mutex<Inner>>,
}

impl Facts {
    pub fn get(&self, key: &str) -> Option<Value> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values
            .get(key)
            .cloned()
    }

    pub fn set(&self, key: impl Into<String>, value: Value) {
        let key = key.into();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.values.get(&key) == Some(&value) {
            return;
        }
        inner.values.insert(key.clone(), value.clone());
        notify(
            &mut inner,
            Change {
                key,
                value: Some(value),
            },
        );
    }

    pub fn remove(&self, key: &str) -> Option<Value> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let old = inner.values.remove(key)?;
        notify(
            &mut inner,
            Change {
                key: key.into(),
                value: None,
            },
        );
        Some(old)
    }

    /// Removes every key starting with `prefix` except those in `keep`, notifying each.
    pub fn remove_prefix(&self, prefix: &str, keep: &[&str]) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<String> = inner
            .values
            .keys()
            .filter(|k| k.starts_with(prefix) && !keep.contains(&k.as_str()))
            .cloned()
            .collect();
        for key in keys {
            inner.values.remove(&key);
            notify(&mut inner, Change { key, value: None });
        }
    }

    /// Exact key or trailing `.*` subtree. Only future changes are sent.
    /// Receivers use unbounded queues; callers should drain or drop them.
    pub fn subscribe(&self, pattern: impl Into<String>) -> Receiver<Change> {
        let (tx, rx) = mpsc::channel();
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subscribers
            .push((pattern.into(), tx));
        rx
    }
}

fn notify(inner: &mut Inner, change: Change) {
    inner.subscribers.retain(|(pattern, tx)| {
        let matches = pattern
            .strip_suffix('*')
            .filter(|p| p.ends_with('.'))
            .map_or(pattern == &change.key, |p| change.key.starts_with(p));
        !matches || tx.send(change.clone()).is_ok()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_filtered_and_deduplicated() {
        let facts = Facts::default();
        let subtree = facts.subscribe("device.*");
        let exact = facts.subscribe("device.a");
        facts.set("device.a", Value::Int(1));
        facts.set("device.a", Value::Int(1));
        facts.set("devices.a", Value::Bool(true));
        assert_eq!(subtree.try_iter().count(), 1);
        assert_eq!(exact.try_iter().count(), 1);
        assert_eq!(facts.remove("device.a"), Some(Value::Int(1)));
        assert_eq!(exact.recv().unwrap().value, None);
        assert!(facts.remove("device.a").is_none());
        assert!(exact.try_recv().is_err());
    }

    #[test]
    fn writers_share_state_and_deliver_every_change() {
        let facts = Facts::default();
        let rx = facts.subscribe("worker.*");
        let threads: Vec<_> = (0..4)
            .map(|i| {
                let facts = facts.clone();
                std::thread::spawn(move || facts.set(format!("worker.{i}"), Value::Int(i)))
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(rx.try_iter().count(), 4);
    }
}
