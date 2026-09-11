use std::collections::VecDeque;
use std::path::Path;

use ade_vm::SshInfo;

/// Captured child output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOut {
    pub stdout: String,
    pub stderr: String,
    /// Raw exit code (`None` = killed by signal).
    pub code: Option<i32>,
}

impl ExecOut {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// The seam Host and MicroVm share: agents only see this, never the
/// concrete environment. `shell()` (pty, M6) opens an interactive shell;
/// `exec` + `ssh_info` is the non-interactive surface.
pub trait WorkspaceProvider: Send {
    fn exec(&mut self, cmd: &[&str], cwd: &Path) -> anyhow::Result<ExecOut>;
    /// `None` = running on the Host. `Some` = attached guest (VM/SSH).
    fn ssh_info(&self) -> Option<SshInfo> {
        None
    }
    /// Interactive shell in `cwd`. Default: unsupported (override per
    /// backend; Host uses `portable-pty`, MicroVm SSH lands in M6f).
    fn shell(&mut self, _cwd: &Path) -> anyhow::Result<Box<dyn ShellChannel>> {
        anyhow::bail!("shell not supported by this provider")
    }
}

/// Interactive shell channel: bytes in/out, resize, liveness, kill.
/// Object-safe so providers return it boxed.
pub trait ShellChannel: Send {
    /// Send keystrokes/bytes to the shell.
    fn write_bytes(&mut self, data: &[u8]) -> anyhow::Result<()>;
    /// Drain all output currently buffered. Never blocks.
    fn read_available(&mut self) -> Vec<u8>;
    /// Resize the pty grid.
    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()>;
    /// Is the child still running?
    fn is_alive(&mut self) -> bool;
    /// Terminate the child.
    fn kill(&mut self) -> anyhow::Result<()>;
}

/// Strip ANSI escape sequences (terminal viewer + agent transcripts).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some('(') | Some(')') | Some('#') => {
                    chars.next();
                }
                Some(_) | None => {}
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// Scripted provider for tests and UI previews: pops canned outputs.
#[derive(Debug, Default)]
pub struct MockWorkspace {
    script: VecDeque<ExecOut>,
    pub executed: Vec<(Vec<String>, std::path::PathBuf)>,
}

impl MockWorkspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, out: ExecOut) {
        self.script.push_back(out);
    }
}

impl WorkspaceProvider for MockWorkspace {
    fn exec(&mut self, cmd: &[&str], cwd: &Path) -> anyhow::Result<ExecOut> {
        self.executed.push((
            cmd.iter().map(|s| s.to_string()).collect(),
            cwd.to_path_buf(),
        ));
        self.script
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("mock workspace: no canned output for {cmd:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_out() -> ExecOut {
        ExecOut {
            stdout: "hi\n".into(),
            stderr: String::new(),
            code: Some(0),
        }
    }

    #[test]
    fn mock_replays_and_records() {
        let mut m = MockWorkspace::new();
        m.enqueue(ok_out());
        let out = m.exec(&["echo", "hi"], Path::new("/tmp")).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(m.executed.len(), 1);
        assert_eq!(m.executed[0].0, vec!["echo".to_string(), "hi".to_string()]);
        // Host default: no SSH.
        assert_eq!(m.ssh_info(), None);
        // Empty script errors instead of hanging.
        assert!(m.exec(&["echo"], Path::new("/")).is_err());
    }

    #[test]
    fn ansi_sequences_are_stripped() {
        assert_eq!(strip_ansi("\x1b[32mgreen\x1b[0m"), "green");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("a\rb"), "ab");
        assert_eq!(strip_ansi("\x1b(Bbold"), "bold");
        assert_eq!(strip_ansi("dangling\x1b"), "dangling");
    }
}
