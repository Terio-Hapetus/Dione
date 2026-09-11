use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;

use super::provider::{ExecOut, ShellChannel, WorkspaceProvider};

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

    fn shell(&mut self, cwd: &Path) -> anyhow::Result<Box<dyn ShellChannel>> {
        Ok(Box::new(HostShell::spawn(cwd)?))
    }
}

/// Local interactive shell over `portable-pty` (M6c). A pump thread moves
/// child output into a channel so `read_available` never blocks the
/// runtime/UI tick; the child is killed on drop.
pub struct HostShell {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    rx: mpsc::Receiver<Vec<u8>>,
}

impl HostShell {
    pub fn spawn(cwd: &Path) -> anyhow::Result<Self> {
        Self::spawn_sized(cwd, 80, 24)
    }

    pub fn spawn_sized(cwd: &Path, cols: u16, rows: u16) -> anyhow::Result<Self> {
        Self::spawn_cmd(cwd, "sh", &[], cols, rows)
    }

    /// Spawn an arbitrary program under the pty (M6f: the ssh client).
    pub fn spawn_cmd(
        cwd: &Path,
        prog: &str,
        args: &[String],
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<Self> {
        let pty = portable_pty::native_pty_system();
        let pair = pty.openpty(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = portable_pty::CommandBuilder::new(prog);
        cmd.cwd(cwd);
        cmd.args(args);
        let child = pair.slave.spawn_command(cmd)?;
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            master: pair.master,
            writer,
            rx,
        })
    }
}

impl ShellChannel for HostShell {
    fn write_bytes(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer
            .write_all(data)
            .and_then(|()| self.writer.flush())
            .map_err(|e| anyhow::anyhow!("pty write: {e:#}"))
    }

    fn read_available(&mut self) -> Vec<u8> {
        self.rx.try_iter().flatten().collect()
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master
            .resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow::anyhow!("pty resize: {e:#}"))
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn kill(&mut self) -> anyhow::Result<()> {
        self.child
            .kill()
            .map_err(|e| anyhow::anyhow!("pty kill: {e:#}"))
    }
}

impl Drop for HostShell {
    fn drop(&mut self) {
        let _ = self.kill();
    }
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

    /// Read with a deadline: pty echo is fast locally, but CI load varies.
    fn read_until(sh: &mut dyn ShellChannel, needle: &str) -> Vec<u8> {
        let mut acc = Vec::new();
        for _ in 0..100 {
            acc.extend(sh.read_available());
            if String::from_utf8_lossy(&acc).contains(needle) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        acc
    }

    #[test]
    fn shell_echo_roundtrip_and_resize() {
        let mut h = HostProvider::new();
        let mut sh = h.shell(Path::new("/tmp")).unwrap();
        assert!(sh.is_alive());
        sh.resize(100, 30).unwrap();
        sh.write_bytes(b"echo hello-pty\n").unwrap();
        let out = read_until(sh.as_mut(), "hello-pty");
        assert!(String::from_utf8_lossy(&out).contains("hello-pty"));
        sh.kill().unwrap();
        // Give the child a moment to exit.
        for _ in 0..20 {
            if !sh.is_alive() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!sh.is_alive());
    }

    #[test]
    fn mock_shell_is_unsupported() {
        use crate::provider::MockWorkspace;

        let mut m = MockWorkspace::new();
        assert!(m.shell(Path::new("/tmp")).is_err());
    }
}
