//! The scheduler thread. It owns the [`Engine`], performs blocking fetches, sleeps
//! until the next Source is due or a command arrives, and publishes an [`Update`]
//! after every wake. No async runtime.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::engine::{ActionOutcome, Alert, Catalog, Engine, EngineState, Presence, RefreshReason};
use crate::push::PushMessage;
use crate::settings::Settings;
use crate::time::Clock;

enum Command {
    Refresh(RefreshReason),
    Presence(Presence),
    Settings(Box<Settings>),
    Push(Box<PushMessage>),
    Action { assistant: String, key: String },
    Shutdown,
}

/// What the scheduler thread publishes after every wake.
#[derive(Clone, Debug)]
pub struct Update {
    pub state: EngineState,
    /// Raised since the previous update; each alert is published once.
    pub alerts: Vec<Alert>,
    pub catalog: Catalog,
    /// Settings actions finished since the previous update; each is published once.
    pub outcomes: Vec<ActionOutcome>,
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

    pub fn apply_settings(&self, settings: Settings) {
        let _ = self.0.send(Command::Settings(Box::new(settings)));
    }

    /// A message from the push channel.
    pub fn push(&self, message: PushMessage) {
        let _ = self.0.send(Command::Push(Box::new(message)));
    }

    /// Runs a module's settings action; its outcome comes with the next [`Update`].
    pub fn run_action(&self, assistant: String, key: String) {
        let _ = self.0.send(Command::Action { assistant, key });
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
        publish: impl Fn(Update) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("scheduler".into())
            .spawn(move || {
                loop {
                    engine.tick();
                    publish(Update {
                        state: engine.state(),
                        alerts: engine.take_alerts(),
                        catalog: engine.catalog(),
                        outcomes: engine.take_outcomes(),
                    });
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
                        Ok(Command::Settings(s)) => engine.apply_settings(&s),
                        Ok(Command::Push(m)) => {
                            if !engine.push(&m) {
                                log::info!("push for {}/{} not accepted", m.assistant, m.source);
                            }
                        }
                        Ok(Command::Action { assistant, key }) => {
                            engine.run_action(&assistant, &key)
                        }
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
