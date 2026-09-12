use std::path::{Path, PathBuf};
use std::process::Command;

use ade_vm::{EphemeralKey, SshInfo};

use super::provider::{ExecOut, ShellChannel, WorkspaceProvider};
use crate::host::HostShell;

/// SSH target for one guest. Built from `VmBackend::wait_ssh` output +
/// the ephemeral key that produced the injected pubkey.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub key_path: PathBuf,
}

/// How a host cwd maps into the guest. Direct passthrough mounts keep
/// absolute host paths (Identity); CloudHypervisor mounts the repo at
/// `/workspace` (Mounted).
#[derive(Debug, Clone)]
pub enum PathMapping {
    Identity,
    Mounted {
        host_root: PathBuf,
        guest_root: PathBuf,
    },
}

impl PathMapping {
    fn map(&self, cwd: &Path) -> anyhow::Result<String> {
        match self {
            Self::Identity => Ok(cwd.to_string_lossy().into_owned()),
            Self::Mounted {
                host_root,
                guest_root,
            } => {
                let rel = cwd.strip_prefix(host_root).map_err(|_| {
                    anyhow::anyhow!(
                        "cwd {} outside mounted {}",
                        cwd.display(),
                        host_root.display()
                    )
                })?;
                Ok(guest_root.join(rel).to_string_lossy().into_owned())
            }
        }
    }
}

/// Agent-facing provider for a booted MicroVM. Backend-agnostic: any
/// guest reachable over ssh with a known key and port works.
/// Secrets ride in the command environment (`env K=V …`), never on disk.
#[derive(Debug)]
pub struct MicroVm {
    target: SshTarget,
    mapping: PathMapping,
    ssh_bin: PathBuf,
    connect_timeout_secs: u64,
    /// `(guest_port, host_port)` preview forwards (`-L`, M6f).
    forwards: Vec<(u16, u16)>,
    _key: Option<EphemeralKey>,
}

impl MicroVm {
    pub fn new(target: SshTarget, mapping: PathMapping) -> Self {
        Self {
            target,
            mapping,
            ssh_bin: PathBuf::from("ssh"),
            connect_timeout_secs: 10,
            forwards: Vec::new(),
            _key: None,
        }
    }

    /// Keep an ephemeral key alive (its tempdir dies with it).
    pub fn attach_ephemeral(mut self, key: EphemeralKey) -> Self {
        self.target.key_path = key.priv_path().to_path_buf();
        self._key = Some(key);
        self
    }

    /// Test seam: fake `ssh` binary.
    pub fn with_ssh_bin(mut self, bin: PathBuf) -> Self {
        self.ssh_bin = bin;
        self
    }

    /// Add a preview forward `host_port:localhost:guest_port` (M6f).
    /// Applied to interactive shells; one-shot `exec` stays forward-free.
    pub fn add_forward(&mut self, guest_port: u16, host_port: u16) {
        self.forwards.push((guest_port, host_port));
    }

    /// Base ssh argv shared by `exec` and `shell` (options only).
    fn base_args(&self) -> Vec<String> {
        vec![
            "-i".into(),
            self.target.key_path.to_string_lossy().into_owned(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            format!("ConnectTimeout={}", self.connect_timeout_secs),
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "-o".into(),
            "UserKnownHostsFile=/dev/null".into(),
            "-p".into(),
            self.target.port.to_string(),
        ]
    }

    fn dest(&self) -> String {
        format!("{}@{}", self.target.user, self.target.host)
    }

    /// `exec` plus secret env injection (`env K=V` prefix).
    pub fn exec_with_env(
        &mut self,
        cmd: &[&str],
        cwd: &Path,
        env: &[(&str, &str)],
    ) -> anyhow::Result<ExecOut> {
        if cmd.is_empty() {
            anyhow::bail!("exec: empty command");
        }
        let dir = self.mapping.map(cwd)?;
        let mut remote = format!("cd {} &&", shell_quote(&dir));
        if !env.is_empty() {
            remote.push_str(" env");
            for (k, v) in env {
                remote.push(' ');
                remote.push_str(k);
                remote.push('=');
                remote.push_str(&shell_quote(v));
            }
        }
        for c in cmd {
            remote.push(' ');
            remote.push_str(&shell_quote(c));
        }
        let mut argv = self.base_args();
        argv.push(self.dest());
        argv.push(remote);
        let out = Command::new(&self.ssh_bin)
            .args(&argv)
            .output()
            .map_err(|e| anyhow::anyhow!("ssh spawn failed: {e:#}"))?;
        Ok(ExecOut {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        })
    }
}

impl WorkspaceProvider for MicroVm {
    fn exec(&mut self, cmd: &[&str], cwd: &Path) -> anyhow::Result<ExecOut> {
        self.exec_with_env(cmd, cwd, &[])
    }

    fn ssh_info(&self) -> Option<SshInfo> {
        Some(SshInfo {
            host: self.target.host.clone(),
            port: self.target.port,
            user: self.target.user.clone(),
        })
    }

    /// Interactive shell: local pty running the ssh client (key
    /// ephemeral, forwards attached). `cwd` must exist locally — for
    /// virtiofs mounts the repo path is host-visible.
    fn shell(&mut self, cwd: &Path) -> anyhow::Result<Box<dyn ShellChannel>> {
        if !cwd.is_dir() {
            anyhow::bail!("shell: cwd {} missing locally", cwd.display());
        }
        let mut argv = self.base_args();
        for (guest, host) in &self.forwards {
            argv.push("-L".into());
            argv.push(format!("{host}:localhost:{guest}"));
        }
        argv.push(self.dest());
        let prog = self.ssh_bin.to_string_lossy().into_owned();
        Ok(Box::new(HostShell::spawn_cmd(cwd, &prog, &argv, 80, 24)?))
    }
}

/// Preview port allocator: one `41xx` host port per forwarded guest port
/// (`VM:3000 → host:41xx`, WORKSPACE-VM spec). Pure, no sockets.
#[derive(Debug, Clone)]
pub struct PreviewPorts {
    next: u16,
}

impl PreviewPorts {
    pub const BASE: u16 = 4100;
    pub const CAP: u16 = 4199;

    pub fn new() -> Self {
        Self { next: Self::BASE }
    }

    pub fn alloc(&mut self) -> anyhow::Result<u16> {
        if self.next > Self::CAP {
            anyhow::bail!("preview ports exhausted ({}-{})", Self::BASE, Self::CAP);
        }
        let p = self.next;
        self.next += 1;
        Ok(p)
    }
}

impl Default for PreviewPorts {
    fn default() -> Self {
        Self::new()
    }
}

pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=+,".contains(c));
    if safe {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// Fake `ssh`: records argv, prints canned output, exits canned code.
    /// Self-contained (absolute paths baked into the script): parallel-safe.
    struct FakeSsh {
        _dir: PathBuf,
        bin: PathBuf,
    }

    static NEXT_FAKE: AtomicU64 = AtomicU64::new(1);

    impl FakeSsh {
        fn new(stdout: &str, code: i32) -> Self {
            use std::sync::atomic::Ordering;
            let dir = std::env::temp_dir().join(format!(
                "ade-fake-ssh-{}-{}",
                std::process::id(),
                NEXT_FAKE.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("out.txt"), stdout).unwrap();
            std::fs::write(dir.join("code.txt"), code.to_string()).unwrap();
            let bin = dir.join("ssh");
            let script = format!(
                "#!/bin/sh\necho \"$@\" > \"{}/args.txt\"\ncat \"{}/out.txt\"\nexit \"$(cat \"{}/code.txt\")\"\n",
                dir.display(),
                dir.display(),
                dir.display()
            );
            std::fs::write(&bin, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            Self { _dir: dir, bin }
        }

        fn args(&self) -> String {
            std::fs::read_to_string(self._dir.join("args.txt")).unwrap()
        }
    }

    impl Drop for FakeSsh {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self._dir);
        }
    }

    fn target() -> SshTarget {
        SshTarget {
            host: "127.0.0.1".into(),
            port: 4222,
            // Same as the live guest user (ade-vm seed.rs).
            user: "ubuntu".into(),
            key_path: PathBuf::from("/tmp/id"),
        }
    }

    #[test]
    fn quote_basics() {
        assert_eq!(shell_quote("echo"), "echo");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn exec_builds_ssh_command() {
        let fake = FakeSsh::new("hello\n", 0);
        let mut vm = MicroVm::new(target(), PathMapping::Identity).with_ssh_bin(fake.bin.clone());
        let out = vm.exec(&["echo", "hi there"], Path::new("/repo")).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "hello\n");
        let argv = fake.args();
        assert!(argv.contains("-p 4222"), "{argv}");
        assert!(argv.contains("ubuntu@127.0.0.1"), "{argv}");
        assert!(argv.contains("cd /repo && echo 'hi there'"), "{argv}");
        // Provider reports the guest (unlike Host's None).
        assert_eq!(vm.ssh_info().unwrap().port, 4222);
    }

    #[test]
    fn env_injection_and_mounted_paths() {
        let fake = FakeSsh::new("", 0);
        let mapping = PathMapping::Mounted {
            host_root: PathBuf::from("/repo"),
            guest_root: PathBuf::from("/workspace"),
        };
        let mut vm = MicroVm::new(target(), mapping).with_ssh_bin(fake.bin.clone());
        vm.exec_with_env(
            &["make", "test"],
            Path::new("/repo/sub"),
            &[("API_KEY", "s3cr3t"), ("EMPTY", "")],
        )
        .unwrap();
        let argv = fake.args();
        assert!(argv.contains("cd /workspace/sub"), "{argv}");
        assert!(argv.contains("env API_KEY=s3cr3t EMPTY=''"), "{argv}");
    }

    #[test]
    fn cwd_outside_mount_errors() {
        let fake = FakeSsh::new("", 0);
        let mapping = PathMapping::Mounted {
            host_root: PathBuf::from("/repo"),
            guest_root: PathBuf::from("/workspace"),
        };
        let mut vm = MicroVm::new(target(), mapping).with_ssh_bin(fake.bin.clone());
        assert!(vm.exec(&["ls"], Path::new("/elsewhere")).is_err());
    }

    #[test]
    fn failing_remote_reports_code() {
        let fake = FakeSsh::new("boom\n", 3);
        let mut vm = MicroVm::new(target(), PathMapping::Identity).with_ssh_bin(fake.bin.clone());
        let out = vm.exec(&["false"], Path::new("/")).unwrap();
        assert!(!out.success());
        assert_eq!(out.code, Some(3));
    }

    /// Interactive fake `ssh`: records argv, then becomes a local shell.
    fn interactive_ssh() -> FakeSsh {
        let fake = FakeSsh::new("", 0);
        let script = format!(
            "#!/bin/sh\necho \"$@\" > \"{}/args.txt\"\nexec sh\n",
            fake._dir.display(),
        );
        std::fs::write(&fake.bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&fake.bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        fake
    }

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
    fn shell_runs_ssh_under_pty_with_forwards() {
        let fake = interactive_ssh();
        let mut vm = MicroVm::new(target(), PathMapping::Identity).with_ssh_bin(fake.bin.clone());
        vm.add_forward(3000, 4105);
        let mut sh = vm.shell(Path::new("/tmp")).unwrap();
        sh.write_bytes(b"echo via-ssh\n").unwrap();
        let out = read_until(&mut *sh, "via-ssh");
        assert!(String::from_utf8_lossy(&out).contains("via-ssh"));
        let argv = fake.args();
        assert!(argv.contains("-i /tmp/id"), "{argv}");
        assert!(argv.contains("-p 4222"), "{argv}");
        assert!(argv.contains("-L 4105:localhost:3000"), "{argv}");
        assert!(argv.contains("ubuntu@127.0.0.1"), "{argv}");
        sh.kill().unwrap();
    }

    #[test]
    fn shell_rejects_missing_local_cwd() {
        let fake = interactive_ssh();
        let mut vm = MicroVm::new(target(), PathMapping::Identity).with_ssh_bin(fake.bin.clone());
        assert!(vm.shell(Path::new("/nonexistent-ade-xyz")).is_err());
    }

    #[test]
    fn preview_ports_allocate_in_range_and_exhaust() {
        let mut ports = PreviewPorts::new();
        assert_eq!(ports.alloc().unwrap(), 4100);
        assert_eq!(ports.alloc().unwrap(), 4101);
        for _ in 0..98 {
            ports.alloc().unwrap();
        }
        assert!(ports.alloc().is_err());
    }
}
