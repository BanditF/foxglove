//! A named worker with queued commands and timed replies.

use crate::{PalmError, Result};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Command<T> = Box<dyn FnOnce(&mut T) -> bool + Send>;

/// A worker that serializes commands on an owned value.
/// Drop waits for queued commands; commands must terminate.
pub struct Actor<T> {
    tx: Option<Sender<Command<T>>>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Actor<T> {
    pub fn spawn(name: &str, mut value: T) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Command<T>>();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                while let Ok(command) = rx.recv() {
                    if !command(&mut value) {
                        break;
                    }
                }
            })
            .map_err(|e| PalmError::Backend(e.to_string()))?;
        Ok(Self {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    /// Runs a command and waits for its reply. A timeout does not cancel running work.
    /// A panic returns Backend and stops the worker; later calls return Disconnected.
    pub fn call<R: Send + 'static>(
        &self,
        timeout: Duration,
        command: impl FnOnce(&mut T) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let (tx, rx) = mpsc::channel();
        let deadline = Instant::now() + timeout;
        self.tx
            .as_ref()
            .ok_or(PalmError::Disconnected)?
            .send(Box::new(move |value| {
                if Instant::now() >= deadline {
                    let _ = tx.send(Err(PalmError::Timeout));
                    return true;
                }
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| command(value))) {
                    Ok(result) => {
                        let _ = tx.send(result);
                        true
                    }
                    Err(_) => {
                        let _ = tx.send(Err(PalmError::Backend("actor command panicked".into())));
                        false
                    }
                }
            }))
            .map_err(|_| PalmError::Disconnected)?;
        rx.recv_timeout(timeout).map_err(|e| match e {
            RecvTimeoutError::Timeout => PalmError::Timeout,
            RecvTimeoutError::Disconnected => PalmError::Disconnected,
        })?
    }
}

impl<T> Drop for Actor<T> {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(2);

    #[test]
    fn commands_serialize_and_panics_disconnect() {
        let actor = Actor::spawn("palm-test", 1).unwrap();
        assert_eq!(
            actor.call(WAIT, |v| {
                *v += 1;
                Ok(*v)
            }),
            Ok(2)
        );
        assert!(matches!(
            actor.call::<()>(WAIT, |_| panic!("test")),
            Err(PalmError::Backend(_))
        ));
        assert_eq!(actor.call(WAIT, |_| Ok(())), Err(PalmError::Disconnected));
    }

    #[test]
    fn timeout_and_drop_join() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let actor = Actor::spawn("palm-timeout", ()).unwrap();
        std::thread::scope(|scope| {
            let call = scope.spawn(|| {
                actor.call(Duration::from_millis(50), move |_| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
            });
            started_rx.recv_timeout(WAIT).unwrap();
            assert_eq!(call.join().unwrap(), Err(PalmError::Timeout));
            release_tx.send(()).unwrap();
        });
        drop(actor);
    }
}
