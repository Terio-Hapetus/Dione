//! M6d terminal adapter: any CLI over a [`ShellChannel`].
//!
//! Unlike `OpencodeAdapter` (API/SSE, exact status), this backend only
//! sees pty bytes: the transcript is shell output and the status is a
//! heuristic — alive means `Working`, exited means `Done`. The UI should
//! read that `Working` as `Working(?)`; [`TerminalAdapter::mark_done`]
//! overrides it when the human (or the CLI's exit) knows better.
//!
//! Permissions are not gated here: the user answers directly in the
//! terminal (the runtime permission overlay stays for opencode tasks).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ade_core::transcript::{Role, TaskId, UnifiedMessage};
use ade_workspace::{ShellChannel, strip_ansi};

use super::agent::{AgentBackend, AgentEvent, AgentStatus};

/// CLI-in-pty backend. Shells are attached by the driver (spawned from
/// the task's `WorkspaceProvider`), keyed by task.
#[derive(Default)]
pub struct TerminalAdapter {
    shells: BTreeMap<TaskId, Box<dyn ShellChannel>>,
    queue: VecDeque<AgentEvent>,
    done: BTreeSet<TaskId>,
    seq: u64,
}

impl TerminalAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand a spawned shell to a task. Replaces any previous shell.
    pub fn attach(&mut self, task: TaskId, shell: Box<dyn ShellChannel>) {
        self.shells.insert(task, shell);
    }

    /// Manual completion override: the next drain reports `Done` even
    /// while the shell is still alive.
    pub fn mark_done(&mut self, task: &TaskId) -> anyhow::Result<()> {
        anyhow::ensure!(self.shells.contains_key(task), "unknown task {task}");
        self.done.insert(*task);
        Ok(())
    }

    fn next_id(&mut self, task: TaskId, kind: &str) -> String {
        self.seq += 1;
        format!("{task}-{kind}{}", self.seq)
    }

    fn send_prompt(&mut self, task: TaskId, text: &str) -> anyhow::Result<()> {
        let shell = self
            .shells
            .get_mut(&task)
            .ok_or_else(|| anyhow::anyhow!("no shell attached for {task}"))?;
        let mut line = text.as_bytes().to_vec();
        line.push(b'\n');
        shell.write_bytes(&line)?;
        let id = self.next_id(task, "u");
        self.queue
            .push_back(AgentEvent::Status(task, AgentStatus::Working));
        self.queue.push_back(AgentEvent::Message(UnifiedMessage {
            id,
            task,
            role: Role::User,
            text: text.to_string(),
            tool: None,
            ts: 0,
        }));
        Ok(())
    }
}

impl AgentBackend for TerminalAdapter {
    fn spawn(&mut self, task: TaskId, prompt: &str) -> anyhow::Result<()> {
        self.send_prompt(task, prompt)
    }

    fn prompt(&mut self, task: &TaskId, text: &str) -> anyhow::Result<()> {
        self.send_prompt(*task, text)
    }

    fn abort(&mut self, task: &TaskId) -> anyhow::Result<()> {
        let shell = self
            .shells
            .get_mut(task)
            .ok_or_else(|| anyhow::anyhow!("unknown task {task}"))?;
        shell.kill()?;
        self.queue
            .push_back(AgentEvent::Status(*task, AgentStatus::Idle));
        Ok(())
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        // Drain new output first so statuses describe the latest state.
        let mut out_ids: Vec<(TaskId, String)> = Vec::new();
        for (task, shell) in self.shells.iter_mut() {
            let raw = shell.read_available();
            if raw.is_empty() {
                continue;
            }
            let text = strip_ansi(&String::from_utf8_lossy(&raw));
            if !text.trim().is_empty() {
                out_ids.push((*task, text));
            }
        }
        for (task, text) in out_ids {
            let id = self.next_id(task, "a");
            self.queue.push_back(AgentEvent::Message(UnifiedMessage {
                id,
                task,
                role: Role::Agent,
                text,
                tool: None,
                ts: 0,
            }));
        }
        let mut statuses: Vec<(TaskId, AgentStatus)> = Vec::new();
        for (task, shell) in self.shells.iter_mut() {
            if self.done.remove(task) || !shell.is_alive() {
                statuses.push((*task, AgentStatus::Done));
            } else {
                statuses.push((*task, AgentStatus::Working));
            }
        }
        for (task, st) in statuses {
            self.queue.push_back(AgentEvent::Status(task, st));
        }
        self.queue.drain(..).collect()
    }

    fn unbind_session(&mut self, _session_id: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct FakeInner {
        written: Vec<u8>,
        outputs: VecDeque<Vec<u8>>,
        alive: bool,
        killed: bool,
    }

    #[derive(Clone, Default)]
    struct FakeHandle {
        inner: Arc<Mutex<FakeInner>>,
    }

    impl FakeHandle {
        fn alive() -> Self {
            Self {
                inner: Arc::new(Mutex::new(FakeInner {
                    alive: true,
                    ..FakeInner::default()
                })),
            }
        }

        fn shell(&self) -> FakeShell {
            FakeShell {
                inner: Arc::clone(&self.inner),
            }
        }

        fn feed(&self, text: &str) {
            self.inner
                .lock()
                .unwrap()
                .outputs
                .push_back(text.as_bytes().to_vec());
        }

        fn kill_shell(&self) {
            self.inner.lock().unwrap().alive = false;
        }
    }

    struct FakeShell {
        inner: Arc<Mutex<FakeInner>>,
    }

    impl ShellChannel for FakeShell {
        fn write_bytes(&mut self, data: &[u8]) -> anyhow::Result<()> {
            self.inner.lock().unwrap().written.extend_from_slice(data);
            Ok(())
        }
        fn read_available(&mut self) -> Vec<u8> {
            self.inner
                .lock()
                .unwrap()
                .outputs
                .drain(..)
                .flatten()
                .collect()
        }
        fn resize(&mut self, _cols: u16, _rows: u16) -> anyhow::Result<()> {
            Ok(())
        }
        fn is_alive(&mut self) -> bool {
            self.inner.lock().unwrap().alive
        }
        fn kill(&mut self) -> anyhow::Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.alive = false;
            g.killed = true;
            Ok(())
        }
    }

    fn attached() -> (TerminalAdapter, FakeHandle, TaskId) {
        let mut ad = TerminalAdapter::new();
        let task = TaskId::new();
        let handle = FakeHandle::alive();
        ad.attach(task, Box::new(handle.shell()));
        (ad, handle, task)
    }

    #[test]
    fn spawn_needs_an_attached_shell() {
        let mut ad = TerminalAdapter::new();
        assert!(ad.spawn(TaskId::new(), "hi").is_err());
        assert!(ad.mark_done(&TaskId::new()).is_err());
        assert!(ad.prompt(&TaskId::new(), "x").is_err());
        assert!(ad.abort(&TaskId::new()).is_err());
    }

    #[test]
    fn prompt_writes_shell_and_queues_events() {
        let (mut ad, handle, task) = attached();
        ad.spawn(task, "hello").unwrap();
        assert_eq!(handle.inner.lock().unwrap().written, b"hello\n");
        let evs = ad.poll();
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Working)));
        assert!(evs.iter().any(|e| matches!(
            e,
            AgentEvent::Message(m) if m.task == task && m.role == Role::User && m.text == "hello"
        )));
    }

    #[test]
    fn output_becomes_stripped_agent_message() {
        let (mut ad, handle, _task) = attached();
        ad.spawn(_task, "hi").unwrap();
        let _ = ad.poll();
        handle.feed("\x1b[32mdone\x1b[0m\n");
        let evs = ad.poll();
        let texts: Vec<&str> = evs
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Message(m) if m.role == Role::Agent => Some(m.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["done\n"]);
    }

    #[test]
    fn dead_shell_reports_done_and_abort_idles() {
        let (mut ad, handle, task) = attached();
        ad.spawn(task, "hi").unwrap();
        let _ = ad.poll();
        handle.kill_shell();
        let evs = ad.poll();
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Done)));

        let (mut ad, handle, task) = attached();
        ad.spawn(task, "hi").unwrap();
        let _ = ad.poll();
        ad.abort(&task).unwrap();
        assert!(handle.inner.lock().unwrap().killed);
        let evs = ad.poll();
        // abort() queues Idle first; the dead shell then also reports Done.
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Idle)));
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Done)));
    }

    #[test]
    fn mark_done_overrides_live_shell_once() {
        let (mut ad, _handle, task) = attached();
        ad.spawn(task, "hi").unwrap();
        let _ = ad.poll();
        ad.mark_done(&task).unwrap();
        let evs = ad.poll();
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Done)));
        // One-shot: next poll falls back to the heuristic.
        let evs = ad.poll();
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Working)));
    }
}
