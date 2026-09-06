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
/// concrete environment. `shell()` (pty, M6) and git-diff-via-mount (M5)
/// extend this trait later; `exec` + `ssh_info` is today's surface.
pub trait WorkspaceProvider: Send {
    fn exec(&mut self, cmd: &[&str], cwd: &Path) -> anyhow::Result<ExecOut>;
    /// `None` = running on the Host. `Some` = attached guest (VM/SSH).
    fn ssh_info(&self) -> Option<SshInfo> {
        None
    }
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
}
