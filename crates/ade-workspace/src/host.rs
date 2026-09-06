use std::path::Path;
use std::process::Command;

use super::provider::{ExecOut, WorkspaceProvider};

/// Runs commands directly on the Host (default mode, Warp-like).
/// Secrets stay in the host keychain; the agent never runs here in
/// VM mode — it only sees the `WorkspaceProvider` face.
#[derive(Debug, Default)]
pub struct HostProvider;

impl HostProvider {
    pub fn new() -> Self {
        Self
    }
}

impl WorkspaceProvider for HostProvider {
    fn exec(&mut self, cmd: &[&str], cwd: &Path) -> anyhow::Result<ExecOut> {
        let (bin, args) = cmd
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("exec: empty command"))?;
        let output = Command::new(bin)
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| anyhow::anyhow!("exec {bin} failed to spawn: {e:#}"))?;
        Ok(ExecOut {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        })
    }
    // ssh_info: default None = Host. No override needed.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_roundtrip() {
        let mut h = HostProvider::new();
        let out = h.exec(&["echo", "hello-host"], Path::new("/tmp")).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout.trim(), "hello-host");
        assert_eq!(h.ssh_info(), None);
    }

    #[test]
    fn empty_command_errors() {
        let mut h = HostProvider::new();
        assert!(h.exec(&[], Path::new("/tmp")).is_err());
    }

    #[test]
    fn failing_command_reports_code() {
        let mut h = HostProvider::new();
        let out = h.exec(&["sh", "-c", "exit 3"], Path::new("/tmp")).unwrap();
        assert!(!out.success());
        assert_eq!(out.code, Some(3));
    }
}
