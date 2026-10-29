//! The scheduler thread. It owns the [`Engine`], performs blocking fetches, sleeps
//! until the next Source is due or a command arrives, and publishes the state after
//! every wake. No async runtime.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::engine::{Engine, EngineState, Presence, RefreshReason};
use crate::time::Clock;

enum Command {
    Refresh(RefreshReason),
    Presence(Presence),
    Shutdown,
}

/// Sends commands to the scheduler thread; cheap to clone, usable from any thread.
#[derive(Clone)]
pub struct EngineHandle(Sender<Command>);

impl EngineHandle {
    pub fn refresh(&self, reason: RefreshReason) {
        let _ = self.0.send(Command::Refresh(reason));
    }

    pub fn set_presence(&self, presence: Presence) {
        let _ = self.0.send(Command::Presence(presence));
    }
}

pub struct EngineThread {
    handle: EngineHandle,
    join: JoinHandle<()>,
}

impl EngineThread {
    pub fn spawn(
        mut engine: Engine,
        clock: Arc<dyn Clock>,
        publish: impl Fn(EngineState) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("scheduler".into())
            .spawn(move || {
                loop {
                    engine.tick();
                    publish(engine.state());
                    let command = match engine.next_wake() {
                        Some(at) => {
                            let secs = (at.0 - clock.now().0).max(1) as u64;
                            rx.recv_timeout(Duration::from_secs(secs))
                        }
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match command {
                        Ok(Command::Refresh(reason)) => engine.request_refresh(reason),
                        Ok(Command::Presence(p)) => engine.set_presence(p),
                        Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .expect("spawn scheduler thread");
        Self {
            handle: EngineHandle(tx),
            join,
        }
    }

    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }

    /// Stops the thread after its current fetch and waits for it.
    pub fn shutdown(self) {
        let _ = self.handle.0.send(Command::Shutdown);
        let _ = self.join.join();
    }
}
