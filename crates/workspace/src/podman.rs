use std::path::{Path, PathBuf};
use std::process::Command;

use super::host::HostShell;
use super::provider::{ExecOut, ShellChannel, WorkspaceProvider};

/// Container workdir. Podman bind-mounts the workspace at this path
/// (`-v <host_root>:<ctr_dir>:rw,Z`), replacing the MicroVM virtiofs
/// `/workspace` mount (ADR-0006). Fresh type on purpose: the old
/// `PathMapping::Mounted` carried SSH/virtiofs semantics we do not inherit.
pub const CTR_WORKSPACE: &str = "/workspace";

/// How a host cwd maps into the container: `host_root` prefix becomes
/// `ctr_dir`. One container per workspace (ADR-0002, engine swapped).
#[derive(Debug, Clone)]
pub struct ContainerMount {
    pub host_root: PathBuf,
    pub ctr_dir: PathBuf,
}

impl ContainerMount {
    pub fn workspace(host_root: &Path) -> Self {
        Self {
            host_root: host_root.to_path_buf(),
            ctr_dir: PathBuf::from(CTR_WORKSPACE),
        }
    }

    fn map(&self, cwd: &Path) -> anyhow::Result<String> {
        let rel = cwd.strip_prefix(&self.host_root).map_err(|_| {
            anyhow::anyhow!(
                "cwd {} outside container mount {}",
                cwd.display(),
                self.host_root.display()
            )
        })?;
        if rel.as_os_str().is_empty() {
            return Ok(self.ctr_dir.to_string_lossy().into_owned());
        }
        Ok(self.ctr_dir.join(rel).to_string_lossy().into_owned())
    }
}

/// Agent-facing provider for a podman container (ADR-0006).
/// Backend-agnostic over the CLI: any `podman`-compatible binary works.
/// Secrets ride in `-e K=V` flags, never on disk. `shell()` lands in P2.
#[derive(Debug)]
pub struct PodmanProvider {
    container: String,
    mount: ContainerMount,
    bin: PathBuf,
}

impl PodmanProvider {
    pub fn new(container: &str, host_root: &Path) -> Self {
        Self {
            container: container.to_string(),
            mount: ContainerMount::workspace(host_root),
            bin: PathBuf::from("podman"),
        }
    }

    /// Test seam: fake `podman` binary.
    pub fn with_bin(mut self, bin: PathBuf) -> Self {
        self.bin = bin;
        self
    }

    pub fn container(&self) -> &str {
        &self.container
    }

    /// `exec` plus secret env injection (`-e K=V` flags).
    pub fn exec_with_env(
        &mut self,
        cmd: &[&str],
        cwd: &Path,
        env: &[(&str, &str)],
    ) -> anyhow::Result<ExecOut> {
        if cmd.is_empty() {
            anyhow::bail!("exec: empty command");
        }
        let dir = self.mount.map(cwd)?;
        let mut argv = vec!["exec".to_string()];
        for (k, v) in env {
            argv.push("-e".to_string());
            argv.push(format!("{k}={v}"));
        }
        argv.push("-w".to_string());
        argv.push(dir);
        argv.push(self.container.clone());
        argv.extend(cmd.iter().map(|s| s.to_string()));
        let out = Command::new(&self.bin)
            .args(&argv)
            .output()
            .map_err(|e| anyhow::anyhow!("podman spawn failed: {e:#}"))?;
        Ok(ExecOut {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        })
    }
}

impl WorkspaceProvider for PodmanProvider {
    fn exec(&mut self, cmd: &[&str], cwd: &Path) -> anyhow::Result<ExecOut> {
        self.exec_with_env(cmd, cwd, &[])
    }
    // shell(): default unsupported until P2.

    /// Interactive shell: local pty running the podman client.
    /// `cwd` maps to the container workdir (`-w`); the pty itself starts
    /// in the host path when it exists (bind source), else temp.
    /// No `-L` forwards here — preview ports publish at `run` (P3).
    fn shell(&mut self, cwd: &Path) -> anyhow::Result<Box<dyn ShellChannel>> {
        let dir = self.mount.map(cwd)?;
        let local = if cwd.is_dir() {
            cwd.to_path_buf()
        } else {
            std::env::temp_dir()
        };
        let bin = self.bin.to_string_lossy().into_owned();
        let args = vec![
            "exec".to_string(),
            "-it".to_string(),
            "-w".to_string(),
            dir,
            self.container.clone(),
            "sh".to_string(),
        ];
        Ok(Box::new(HostShell::spawn_cmd(&local, &bin, &args, 80, 24)?))
    }
}

/// Is `podman` runnable via `PATH`? Missing → Host fallback + banner.
pub fn probe_podman() -> bool {
    crate::agents::probe_bin("podman")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// Fake `podman`: records argv, prints canned output, exits canned code.
    /// Self-contained (absolute paths baked into the script): parallel-safe.
    struct FakePodman {
        _dir: PathBuf,
        bin: PathBuf,
    }

    static NEXT_FAKE: AtomicU64 = AtomicU64::new(1);

    impl FakePodman {
        fn new(stdout: &str, code: i32) -> Self {
            use std::sync::atomic::Ordering;
            let dir = std::env::temp_dir().join(format!(
                "dione-fake-podman-{}-{}",
                std::process::id(),
                NEXT_FAKE.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("out.txt"), stdout).unwrap();
            std::fs::write(dir.join("code.txt"), code.to_string()).unwrap();
            let bin = dir.join("podman");
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

    impl Drop for FakePodman {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self._dir);
        }
    }

    fn provider(fake: &FakePodman) -> PodmanProvider {
        PodmanProvider::new("dione-test", Path::new("/repo")).with_bin(fake.bin.clone())
    }

    #[test]
    fn exec_builds_podman_command() {
        let fake = FakePodman::new("hello\n", 0);
        let mut p = provider(&fake);
        let out = p.exec(&["echo", "hi there"], Path::new("/repo")).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "hello\n");
        let argv = fake.args();
        assert!(
            argv.contains("exec -w /workspace dione-test echo hi there"),
            "{argv}"
        );
    }

    #[test]
    fn env_injection_and_subdir_mapping() {
        let fake = FakePodman::new("", 0);
        let mut p = provider(&fake);
        let out = p
            .exec_with_env(
                &["printenv", "K"],
                Path::new("/repo/sub"),
                &[("K", "V"), ("A B", "x")],
            )
            .unwrap();
        assert!(out.success());
        let argv = fake.args();
        assert!(argv.contains("-e K=V"), "{argv}");
        assert!(argv.contains("-e A B=x"), "{argv}");
        assert!(argv.contains("-w /workspace/sub"), "{argv}");
    }

    #[test]
    fn cwd_outside_mount_is_a_clean_error() {
        let fake = FakePodman::new("", 0);
        let mut p = provider(&fake);
        assert!(p.exec(&["echo"], Path::new("/elsewhere")).is_err());
        assert!(p.exec(&[], Path::new("/repo")).is_err());
    }

    #[test]
    fn probe_never_panics() {
        // Either answer is fine; the point is Host fallback, not podman.
        let _ = probe_podman();
    }

    /// Interactive fake `podman`: records argv, then becomes a local shell.
    fn interactive_podman() -> FakePodman {
        let fake = FakePodman::new("", 0);
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
        // Generous budget: pty spawn stalls under full-workspace parallel runs
        // (same flake class as the legacy fake-ssh shell test).
        for _ in 0..200 {
            acc.extend(sh.read_available());
            if String::from_utf8_lossy(&acc).contains(needle) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        acc
    }

    #[test]
    fn shell_runs_podman_exec_under_pty() {
        let fake = interactive_podman();
        let mut p = provider(&fake);
        let mut sh = p.shell(Path::new("/repo/sub-missing")).unwrap();
        sh.write_bytes(b"echo via-podman\n").unwrap();
        let out = read_until(&mut *sh, "via-podman");
        assert!(String::from_utf8_lossy(&out).contains("via-podman"));
        let argv = fake.args();
        assert!(
            argv.contains("exec -it -w /workspace/sub-missing"),
            "{argv}"
        );
        assert!(argv.contains("dione-test sh"), "{argv}");
        // No SSH legacy: no -i/-p/-L/user@host anywhere.
        assert!(!argv.contains("-L"), "{argv}");
        assert!(!argv.contains('@'), "{argv}");
        sh.kill().unwrap();
    }

    #[test]
    fn shell_rejects_cwd_outside_mount() {
        let fake = interactive_podman();
        let mut p = provider(&fake);
        assert!(p.shell(Path::new("/elsewhere")).is_err());
    }
}
