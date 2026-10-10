//! A session's script thread: every engine of the session lives on it, and every request to them
//! goes through one ordered queue (the C# worker process and its queue). A load, a stop and the
//! events after them are handled in the order they were sent, so an event sent after a script
//! asked to load reaches it once it runs. The thread wakes the UI after each reply.
//!
//! A panic on the thread ends it: it replies [`Reply::Failed`] once and exits, and the session
//! applies the restart policy. The engines it held are leaked rather than dropped, since a panic
//! may have left one half way through a call.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use super::engines::EngineSet;
use super::{Runtime, ScriptEvent, ScriptResult};

/// Wakes the UI when replies are ready.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Stack of the script thread (QuickJS's own limit is far below it).
const STACK: usize = 8 * 1024 * 1024;

pub enum Request {
    /// The host's protocol cache, for the running engines and the ones loaded later.
    Seed(String),
    Load {
        ticket: u64,
        id: String,
        source: String,
        restricted_send: bool,
        /// The script's language and layer.
        runtime: Runtime,
    },
    /// Events for the named engines: every event to every target, events in order.
    Dispatch {
        ticket: u64,
        targets: Vec<String>,
        events: Vec<ScriptEvent>,
    },
    Stop(String),
    /// Tests: the thread panics, as a bug in it would.
    #[doc(hidden)]
    Panic,
}

/// One reply per request, except [`Request::Stop`], which needs none.
#[derive(Debug)]
pub enum Reply {
    Seeded {
        failed: Vec<(String, ScriptResult)>,
    },
    Loaded {
        ticket: u64,
        result: ScriptResult,
    },
    /// The results that are not empty, as (event index, target index, result).
    Dispatched {
        ticket: u64,
        results: Vec<(usize, usize, ScriptResult)>,
    },
    /// The thread failed and has exited; every engine is gone.
    Failed(String),
}

pub struct ScriptThread {
    requests: Option<Sender<Request>>,
    replies: Receiver<Reply>,
    handle: Option<JoinHandle<()>>,
    /// Events sent and not yet taken up by the thread (the C# worker's pending count).
    waiting: Arc<AtomicUsize>,
}

impl ScriptThread {
    pub fn spawn(name: &str, wake: Wake) -> std::io::Result<Self> {
        let (requests, inbox) = channel::<Request>();
        let (replies, outbox) = channel::<Reply>();
        let waiting = Arc::new(AtomicUsize::new(0));
        let taken = Arc::clone(&waiting);
        let handle = thread::Builder::new()
            .name(name.into())
            .stack_size(STACK)
            .spawn(move || run(inbox, replies, wake, taken))?;
        Ok(Self {
            requests: Some(requests),
            replies: outbox,
            handle: Some(handle),
            waiting,
        })
    }

    /// Queue a request. False when the thread has gone.
    pub fn send(&self, request: Request) -> bool {
        let events = match &request {
            Request::Dispatch { events, .. } => events.len(),
            _ => 0,
        };
        self.waiting.fetch_add(events, Ordering::Relaxed);
        let sent = self.requests.as_ref().is_some_and(|r| r.send(request).is_ok());
        if !sent {
            self.waiting.fetch_sub(events, Ordering::Relaxed);
        }
        sent
    }

    /// Events sent that the thread has not started on yet.
    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Relaxed)
    }

    /// The next reply, if any.
    pub fn poll(&self) -> Option<Reply> {
        self.replies.try_recv().ok()
    }

    /// Stop waiting for the thread: it ends after the call it is in (every call is limited),
    /// without anyone joining it.
    pub fn abandon(mut self) {
        self.requests = None;
        self.handle = None;
    }
}

impl Drop for ScriptThread {
    fn drop(&mut self) {
        self.requests = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run(inbox: Receiver<Request>, replies: Sender<Reply>, wake: Wake, waiting: Arc<AtomicUsize>) {
    let mut engines = EngineSet::default();
    while let Ok(request) = inbox.recv() {
        if let Request::Dispatch { events, .. } = &request {
            waiting.fetch_sub(events.len(), Ordering::Relaxed);
        }
        let handled = catch_unwind(AssertUnwindSafe(|| handle(&mut engines, request)));
        match handled {
            Ok(None) => {}
            Ok(Some(reply)) => {
                if replies.send(reply).is_err() {
                    break;
                }
                wake();
            }
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                std::mem::forget(engines);
                let _ = replies.send(Reply::Failed(message));
                wake();
                return;
            }
        }
    }
}

fn handle(engines: &mut EngineSet, request: Request) -> Option<Reply> {
    match request {
        Request::Seed(state) => Some(Reply::Seeded {
            failed: engines.seed(&state),
        }),
        Request::Load {
            ticket,
            id,
            source,
            restricted_send,
            runtime,
        } => Some(Reply::Loaded {
            ticket,
            result: engines.load_with(&id, &source, restricted_send, runtime),
        }),
        Request::Dispatch {
            ticket,
            targets,
            events,
        } => {
            let mut results = Vec::new();
            for (e, event) in events.iter().enumerate() {
                for (t, id) in targets.iter().enumerate() {
                    let result = engines.dispatch_one(id, event);
                    if !result.is_empty() {
                        results.push((e, t, result));
                    }
                }
            }
            Some(Reply::Dispatched { ticket, results })
        }
        Request::Stop(id) => {
            engines.stop(&id);
            None
        }
        Request::Panic => panic!("script thread test panic"),
    }
}

#[cfg(all(test, feature = "scripting"))]
mod tests {
    use super::*;
    use crate::scripting::{ActionKind, EventKind, ScriptAction};
    use std::time::{Duration, Instant};

    fn wait(thread: &ScriptThread) -> Reply {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(reply) = thread.poll() {
                return reply;
            }
            assert!(Instant::now() < deadline, "no reply");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// C# `RealWorkerKeepsStateAndDiscardsOnlyTheScriptThatFailed` and the Unicode case.
    #[test]
    fn the_thread_keeps_state_discards_only_the_failed_script_and_keeps_unicode() {
        let thread = ScriptThread::spawn("test-scripts", Arc::new(|| {})).unwrap();
        let load = |ticket, id: &str, source: &str| Request::Load {
            ticket,
            id: id.into(),
            source: source.into(),
            restricted_send: false,
            runtime: Runtime::JAVASCRIPT,
        };
        let dispatch = |ticket, targets: &[&str], text: &str| Request::Dispatch {
            ticket,
            targets: targets.iter().map(|s| s.to_string()).collect(),
            events: vec![ScriptEvent::new(EventKind::Command, text)],
        };
        thread.send(load(
            1,
            "a",
            "let n=0; mud.alias('x', () => mud.echo(String(++n))); mud.alias('bad', () => { mud.send('discard'); throw Error('boom') });",
        ));
        thread.send(load(2, "b", "mud.alias('x', () => mud.echo('b'));"));
        assert!(matches!(wait(&thread), Reply::Loaded { ticket: 1, result } if result.error.is_none()));
        assert!(matches!(wait(&thread), Reply::Loaded { ticket: 2, .. }));
        thread.send(dispatch(3, &["a"], "x"));
        thread.send(dispatch(4, &["a", "b"], "bad"));
        thread.send(dispatch(5, &["a", "b"], "x"));
        let Reply::Dispatched { results, .. } = wait(&thread) else {
            panic!()
        };
        assert_eq!(results[0].2.actions, [ScriptAction::new(ActionKind::Echo, "1")]);
        let Reply::Dispatched { results, .. } = wait(&thread) else {
            panic!()
        };
        assert_eq!(results.len(), 1, "b has no 'bad' alias");
        assert!(results[0].2.error.as_deref().unwrap().contains("boom"));
        assert!(results[0].2.actions.is_empty());
        let Reply::Dispatched { results, .. } = wait(&thread) else {
            panic!()
        };
        assert_eq!(results.len(), 1);
        assert_eq!(
            (results[0].1, &results[0].2.actions),
            (1, &vec![ScriptAction::new(ActionKind::Echo, "b")])
        );

        thread.send(load(
            6,
            "u",
            "mud.alias(/^(.+)$/, m => { mud.echo('こんにちは 🌍 ' + m[1]); mud.send('examiner café'); });",
        ));
        assert!(matches!(wait(&thread), Reply::Loaded { ticket: 6, .. }));
        thread.send(dispatch(7, &["u"], "Grüße 世界 🧙"));
        let Reply::Dispatched { results, .. } = wait(&thread) else {
            panic!()
        };
        assert!(results[0].2.handled);
        assert_eq!(
            results[0].2.actions,
            [
                ScriptAction::new(ActionKind::Echo, "こんにちは 🌍 Grüße 世界 🧙"),
                ScriptAction::new(ActionKind::Send, "examiner café"),
            ]
        );
    }

    /// C# `RealWorkerCarriesPanelActionsAndHonoursTheRestrictedSendPolicy`.
    #[test]
    fn panel_actions_and_the_restricted_send_policy() {
        let mut set = EngineSet::default();
        let loaded = set.load(
            "ship",
            r#"
            const p = mud.panel("ship", { title: "Ship" });
            p.gauge("hull", { label: "Hull", value: 12, max: 100 });
            p.button("flee", { onClick: () => mud.send("flee") });
            mud.trigger(/^hit$/, () => mud.send("flee"));
            "#,
            true,
        );
        assert_eq!(loaded.error, None);
        let panel = |text: &str| ScriptAction::new(ActionKind::Panel, text);
        assert_eq!(
            loaded.actions,
            [
                panel(r#"{"panel":"ship","action":"create","title":"Ship","dock":"right"}"#),
                panel(
                    r#"{"panel":"ship","action":"widget","widget":"hull","kind":"gauge","props":{"label":"Hull","value":12,"max":100}}"#
                ),
                panel(r#"{"panel":"ship","action":"widget","widget":"flee","kind":"button","props":{"label":"flee"}}"#),
            ]
        );
        let click = set.dispatch_one(
            "ship",
            &ScriptEvent::new(
                EventKind::Panel,
                r#"{"panel":"ship","widget":"flee","event":"click","value":null}"#,
            ),
        );
        assert_eq!(click.actions, [ScriptAction::new(ActionKind::Send, "flee")]);
        let refused = set.dispatch_one("ship", &ScriptEvent::new(EventKind::Line, "hit"));
        assert!(refused.error.as_deref().unwrap().contains("Pack send policy"));
        assert!(refused.actions.is_empty());
    }

    #[test]
    fn a_panic_ends_the_thread_with_one_failure() {
        let thread = ScriptThread::spawn("test-scripts-panic", Arc::new(|| {})).unwrap();
        thread.send(Request::Panic);
        assert!(matches!(wait(&thread), Reply::Failed(_)));
        std::thread::sleep(Duration::from_millis(20));
        assert!(!thread.send(Request::Seed("{}".into())), "the thread has gone");
        assert!(thread.poll().is_none());
    }
}
