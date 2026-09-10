use std::path::{Path, PathBuf};
use std::process::Command;

use ade_vm::{EphemeralKey, SshInfo};

use super::provider::{ExecOut, WorkspaceProvider};

/// SSH target for one guest. Built from `VmBackend::wait_ssh` output +
/// the ephemeral key that produced the injected pubkey.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub key_path: PathBuf,
}

/// How a host cwd maps into the guest. sbx mounts absolute host paths
/// (Identity); CloudHypervisor mounts the repo at `/workspace` (Mounted).
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

/// Agent-facing provider for a booted MicroVM. Backend-agnostic: works
/// for sbx (`<name>.sbx`) and CloudHypervisor (vsock-proxy port) alike.
/// Secrets ride in the command environment (`env K=V …`), never on disk.
#[derive(Debug)]
pub struct MicroVm {
    target: SshTarget,
    mapping: PathMapping,
    ssh_bin: PathBuf,
    connect_timeout_secs: u64,
    _key: Option<EphemeralKey>,
}

impl MicroVm {
    pub fn new(target: SshTarget, mapping: PathMapping) -> Self {
        Self {
            target,
            mapping,
            ssh_bin: PathBuf::from("ssh"),
            connect_timeout_secs: 10,
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
        let dest = format!("{}@{}", self.target.user, self.target.host);
        let out = Command::new(&self.ssh_bin)
            .args([
                "-i",
                &self.target.key_path.to_string_lossy(),
                "-o",
                "BatchMode=yes",
                "-o",
                &format!("ConnectTimeout={}", self.connect_timeout_secs),
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-p",
                &self.target.port.to_string(),
                &dest,
                &remote,
            ])
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
            user: "vm".into(),
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
        assert!(argv.contains("vm@127.0.0.1"), "{argv}");
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
}
