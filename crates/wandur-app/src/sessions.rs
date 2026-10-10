//! The open sessions and the actions panels can ask of the app.

use std::time::Instant;

use wandur_core::{Endpoint, Waker};

use crate::session_tab::{SessionId, SessionTab, TabOptions};
use crate::terminal_view::TerminalViewState;

/// Something a panel asks the app to do after drawing (so no panel needs `&mut App`).
#[derive(Clone, Debug, PartialEq)]
pub enum AppAction {
    Connect(Endpoint),
    /// Connect to saved world number n (always a new session: toolbar Connect, Connect in new tab).
    ConnectWorld(usize),
    /// Go to saved world n's open session, or connect when it has none (Saved worlds: a
    /// double-click, Enter, Connect).
    GoToWorld(usize),
    /// Add a copy of saved world n after it, without its saved password.
    DuplicateWorld(usize),
    /// Show saved world n's page in Find a MUD (when the directory lists it).
    ExploreWorld(usize),
    /// Save the session's endpoint as a world.
    SaveWorld(SessionId),
    Focus(SessionId),
    Disconnect(SessionId),
    Reconnect(SessionId),
    Close(SessionId),
    /// Open another session to the same world (a session tab's Duplicate session).
    Duplicate(SessionId),
    /// Close Find a MUD (its tab's close button, Cmd+W while it is shown).
    CloseDirectory,
    /// Close every document tab but this one (a tab's Close other tabs).
    CloseOthers(crate::workspace::Tab),
    /// Move a document tab to this index of the tab strip (a drag).
    MoveTab(crate::workspace::Tab, usize),
    /// The map editor deleted rooms or exits on a session's map (the undo toast offers Undo).
    MapDeleted(SessionId, crate::map_view::editor::Deleted),
    /// Undo what the toast offers (the toast with this number, if it is still shown).
    UndoToast(u64),
    /// File > Import map, for this session's world when given (the full map's Import).
    OpenMapImport(Option<SessionId>),
    /// Name a session (empty: back to the world's name).
    Rename(SessionId, String),
    /// Send a command on a session (a channel reply).
    SendCommand(SessionId, String),
    OpenSettings,
    /// Show a session's full map (its Map page) with Edit on.
    OpenMapEditor(SessionId),
    /// Show a session's full map (its Map page).
    OpenFullMap(SessionId),
    /// Play and Map side by side settled on this map share (remembered for new sessions).
    RememberSplit(f32),
    /// Install a room classifier package from this zip or folder (off the UI thread).
    InstallClassifier(std::path::PathBuf),
    /// Open the offline demo world in a new session.
    OpenDemo,
    /// Settings changed in the Settings tab: apply and save.
    SettingsChanged,
    /// Show the directory (Find a MUD).
    OpenDirectory,
    /// Open or focus a panel.
    OpenPanel(crate::workspace::Tab),
    /// Take a tool panel out of the dock onto its edge's strip (auto hide).
    Unpin(crate::workspace::Tab),
    /// Put an auto-hidden panel back where it was.
    Pin(crate::workspace::Tab),
    ResetLayout,
    RefreshDirectory,
    ShowAdult(bool),
    /// Add a directory listing to the saved worlds, and connect if asked.
    SaveListing {
        id: String,
        tls: bool,
        connect: bool,
    },
    NewWorld,
    /// Open a web address the person confirmed in a transcript's link bar.
    OpenLink(String),
    /// Show a message in the toolbar (a refused link).
    Notice(String),
    EditWorld(usize),
    /// The world editor's Scripts section for a session's world (Scripts menu, Session menu).
    EditScripts(SessionId),
    /// Read the session's world library again and restart its scripts (Reload saved rules).
    ReloadScripts(SessionId),
    /// The world editor's Agent settings section for a session's world (Configure agent...).
    EditAgent(SessionId),
    DeleteWorld(usize),
    /// Don't show again on the history reminder.
    HideHistoryNotice,
    /// Close a session's notice strip.
    DismissNotice(SessionId),
    /// View > Session history.
    OpenHistory,
    /// Mark as channel... on a transcript line: the session, the line and a second example.
    MarkChannel(SessionId, String, Option<String>),
}

pub struct SessionEntry {
    pub tab: SessionTab,
    pub view: TerminalViewState,
}

#[derive(Default)]
pub struct Sessions {
    entries: Vec<SessionEntry>,
    next_id: SessionId,
    /// Server characters applied across all sessions since start (for the probe).
    pub chars_total: u64,
}

impl Sessions {
    pub fn open(&mut self, endpoint: Endpoint, options: &TabOptions, waker: Waker) -> SessionId {
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(SessionEntry {
            tab: SessionTab::open(id, endpoint, options, waker),
            view: TerminalViewState::default(),
        });
        id
    }

    /// Open the offline demo world.
    pub fn open_demo(&mut self, options: &TabOptions) -> SessionId {
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(SessionEntry {
            tab: SessionTab::demo(id, options),
            view: TerminalViewState::default(),
        });
        id
    }

    pub fn get(&self, id: SessionId) -> Option<&SessionEntry> {
        self.entries.iter().find(|e| e.tab.id == id)
    }

    pub fn get_mut(&mut self, id: SessionId) -> Option<&mut SessionEntry> {
        self.entries.iter_mut().find(|e| e.tab.id == id)
    }

    /// Drop a session; its connection closes.
    pub fn close(&mut self, id: SessionId) {
        self.entries.retain(|e| e.tab.id != id);
    }

    /// Put the sessions in this order (the session tabs'); sessions not named keep their place
    /// after the named ones.
    pub fn reorder(&mut self, order: &[SessionId]) {
        let rank = |id: SessionId| order.iter().position(|o| *o == id).unwrap_or(usize::MAX);
        self.entries.sort_by_key(|e| rank(e.tab.id));
    }

    pub fn iter(&self) -> impl Iterator<Item = &SessionEntry> {
        self.entries.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut SessionEntry> {
        self.entries.iter_mut()
    }

    /// Apply network output and timers to every session, visible or not. Returns characters applied.
    pub fn pump_all(&mut self, now: Instant) -> usize {
        let applied: usize = self.entries.iter_mut().map(|e| e.tab.pump(now)).sum();
        self.chars_total += applied as u64;
        applied
    }

    /// The earliest time a session needs a frame without input (reconnect or prompt timers).
    pub fn deadline(&self) -> Option<Instant> {
        self.entries.iter().filter_map(|e| e.tab.deadline()).min()
    }

    pub fn connected(&self) -> usize {
        self.entries.iter().filter(|e| e.tab.is_connected()).count()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn output_of_several_sessions_stays_apart_and_marks_activity_per_session() {
        let mut sessions = Sessions::default();
        let mut servers = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..3 {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
            ids.push(sessions.open(endpoint, &TabOptions::default(), Arc::new(|| {})));
            servers.push(listener.accept().unwrap().0);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while sessions.connected() < 3 {
            assert!(Instant::now() < deadline);
            sessions.pump_all(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
        for id in &ids {
            sessions.get_mut(*id).unwrap().tab.mark_seen();
        }
        // Session 0 gets 5 lines, session 2 gets 1, session 1 nothing.
        for i in 0..5 {
            servers[0].write_all(format!("zero {i}\r\n").as_bytes()).unwrap();
        }
        servers[2].write_all(b"two\r\n").unwrap();
        while sessions.get(ids[0]).unwrap().tab.unseen_lines() < 5
            || sessions.get(ids[2]).unwrap().tab.unseen_lines() < 1
        {
            assert!(Instant::now() < deadline);
            sessions.pump_all(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
        let text = |i: usize| sessions.get(ids[i]).unwrap().tab.terminal.transcript();
        assert!(text(0).contains("zero 4") && !text(0).contains("two"));
        assert!(text(2).ends_with("two") && !text(2).contains("zero"));
        assert_eq!(sessions.get(ids[0]).unwrap().tab.unseen_lines(), 5);
        assert!(!sessions.get(ids[1]).unwrap().tab.has_unseen());
        assert_eq!(sessions.get(ids[2]).unwrap().tab.unseen_lines(), 1);
        assert!(sessions.chars_total >= 5 * 8 + 5);
        sessions.close(ids[1]);
        assert_eq!(sessions.len(), 2);
    }
}
