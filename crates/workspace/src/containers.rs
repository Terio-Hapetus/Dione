//! P3 containers: lifecycle over the podman CLI (ADR-0006).
//!
//! One container per workspace (ADR-0002, engine swapped): the manager
//! ensures a long-lived `sleep infinity` container with the workspace
//! bind-mounted at [`crate::podman::CTR_WORKSPACE`]. Agents then run via
//! `PodmanProvider` (`exec`/`shell`). Only communication is CLI argv —
//! no SSH, no vsock, no key material.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned guest image (digest-pin in production configs, tag here).
pub const CTR_IMAGE: &str = "docker.io/library/ubuntu:24.04";
/// Guest dev-server port published as `host 41xx` (preview convention).
pub const PREVIEW_CTR_PORT: u16 = 3000;

/// Lifecycle states. No `WaitingSsh`/`Mounting` — there is no guest
/// SSH or virtiofs mount to wait for (anti-legacy rule, ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Missing,
    Pulling,
    Running,
    Stopped,
    Error(String),
}

/// What to run: name derived from [`container_name_for`], image pinned.
#[derive(Debug, Clone)]
pub struct ContainerSpec {
    pub name: String,
    pub host_root: PathBuf,
    pub image: String,
    pub preview_host: Option<u16>,
}

impl ContainerSpec {
    pub fn for_workspace(host_root: &Path) -> Self {
        Self {
            name: container_name_for(host_root),
            host_root: host_root.to_path_buf(),
            image: CTR_IMAGE.to_string(),
            preview_host: None,
        }
    }
}

/// Deterministic, podman-safe name (`dione-` + 12 hex of the path hash).
pub fn container_name_for(root: &Path) -> String {
    let mut h = DefaultHasher::new();
    root.to_string_lossy().hash(&mut h);
    format!("dione-{:012x}", h.finish() & 0xffffffffffff)
}

/// CLI driver. Synchronous: `ensure_running` blocks until the container
/// exists (the UI thread calls it from a background thread, P4).
#[derive(Debug)]
pub struct ContainerManager {
    bin: PathBuf,
}

impl ContainerManager {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from("podman"),
        }
    }

    /// Test seam: fake `podman` binary.
    pub fn with_bin(mut self, bin: PathBuf) -> Self {
        self.bin = bin;
        self
    }

    fn run_cli(&self, args: &[String]) -> anyhow::Result<String> {
        let out = Command::new(&self.bin)
            .args(args)
            .output()
            .map_err(|e| anyhow::anyhow!("podman spawn failed: {e:#}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "podman {} failed: {}",
                args.first().map(String::as_str).unwrap_or("?"),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Pure argv for `run` (unit-tested shape; executed by `start`).
    pub fn run_argv(&self, spec: &ContainerSpec) -> Vec<String> {
        let mut argv = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            spec.name.clone(),
            "--cap-drop=all".to_string(),
            "--security-opt=no-new-privileges".to_string(),
            "--pids-limit=256".to_string(),
            "--memory=2g".to_string(),
            "--cpus=2".to_string(),
            "--read-only".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "-v".to_string(),
            format!("{}:/workspace:rw,Z", spec.host_root.to_string_lossy()),
            "-w".to_string(),
            "/workspace".to_string(),
            "--hostname".to_string(),
            "dione".to_string(),
            "--network".to_string(),
            "slirp4netns".to_string(),
        ];
        if let Some(host) = spec.preview_host {
            argv.push("-p".to_string());
            argv.push(format!("{host}:{}", PREVIEW_CTR_PORT));
        }
        argv.push(spec.image.clone());
        argv.push("sleep".to_string());
        argv.push("infinity".to_string());
        argv
    }

    pub fn exists(&self, name: &str) -> anyhow::Result<bool> {
        let out = self.run_cli(&[
            "ps".to_string(),
            "-a".to_string(),
            "--filter".to_string(),
            format!("name=^{name}$"),
            "--format".to_string(),
            "{{.Names}}".to_string(),
        ])?;
        Ok(out.lines().any(|l| l.trim() == name))
    }

    pub fn pull(&self, image: &str) -> anyhow::Result<()> {
        self.run_cli(&["pull".to_string(), image.to_string()])?;
        Ok(())
    }

    pub fn start(&self, spec: &ContainerSpec) -> anyhow::Result<()> {
        self.run_cli(&self.run_argv(spec))?;
        Ok(())
    }

    pub fn stop(&self, name: &str) -> anyhow::Result<bool> {
        let out = self.run_cli(&[
            "ps".to_string(),
            "-a".to_string(),
            "--filter".to_string(),
            format!("name=^{name}$"),
            "--format".to_string(),
            "{{.Names}}".to_string(),
        ])?;
        if !out.lines().any(|l| l.trim() == name) {
            return Ok(false);
        }
        self.run_cli(&["rm".to_string(), "-f".to_string(), name.to_string()])?;
        Ok(true)
    }

    /// Missing → pull → run → `Running`. Existing → `Running` untouched.
    /// Failures land in `Error` for manual retry — never auto-pruned.
    pub fn ensure_running(&self, spec: &ContainerSpec) -> ContainerState {
        match self.exists(&spec.name) {
            Err(e) => return ContainerState::Error(format!("ps failed: {e:#}")),
            Ok(true) => return ContainerState::Running,
            Ok(false) => {}
        }
        if let Err(e) = self.pull(&spec.image) {
            return ContainerState::Error(format!("pull failed: {e:#}"));
        }
        match self.start(spec) {
            Ok(()) => ContainerState::Running,
            Err(e) => ContainerState::Error(format!("run failed: {e:#}")),
        }
    }
}

impl Default for ContainerManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake `podman` CLI: `ps` prints canned names, everything else prints
    /// canned output; exit codes from files. Records every argv.
    struct FakePodman {
        _dir: PathBuf,
        bin: PathBuf,
    }

    impl FakePodman {
        fn new(ps_out: &str, out: &str, code: i32) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let dir = std::env::temp_dir().join(format!(
                "dione-fake-podcli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("ps.txt"), ps_out).unwrap();
            std::fs::write(dir.join("out.txt"), out).unwrap();
            std::fs::write(dir.join("code.txt"), code.to_string()).unwrap();
            let bin = dir.join("podman");
            let script = format!(
                "#!/bin/sh\necho \"$@\" >> \"{0}/calls.txt\"\nif [ \"$1\" = \"ps\" ]; then cat \"{0}/ps.txt\"; else cat \"{0}/out.txt\"; fi\nexit \"$(cat \"{0}/code.txt\")\"\n",
                dir.display(),
            );
            std::fs::write(&bin, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            Self { _dir: dir, bin }
        }

        fn calls(&self) -> String {
            std::fs::read_to_string(self._dir.join("calls.txt")).unwrap_or_default()
        }
    }

    impl Drop for FakePodman {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self._dir);
        }
    }

    fn mgr(fake: &FakePodman) -> ContainerManager {
        ContainerManager::new().with_bin(fake.bin.clone())
    }

    #[test]
    fn names_are_deterministic_safe_and_scoped() {
        let a = container_name_for(Path::new("/repo/a"));
        let b = container_name_for(Path::new("/repo/a"));
        let c = container_name_for(Path::new("/repo/b"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        for n in [&a, &c] {
            assert!(n.starts_with("dione-"), "{n}");
            assert!(
                n.chars().all(|x| x.is_ascii_alphanumeric() || x == '-'),
                "{n}"
            );
        }
    }

    #[test]
    fn run_argv_carries_hardening_and_mount() {
        let m = ContainerManager::new();
        let mut spec = ContainerSpec::for_workspace(Path::new("/repo/ws"));
        spec.preview_host = Some(4105);
        let argv = m.run_argv(&spec).join(" ");
        assert!(argv.contains("--cap-drop=all"), "{argv}");
        assert!(argv.contains("--security-opt=no-new-privileges"), "{argv}");
        assert!(argv.contains("--read-only"), "{argv}");
        assert!(argv.contains("-v /repo/ws:/workspace:rw,Z"), "{argv}");
        assert!(argv.contains("-p 4105:3000"), "{argv}");
        assert!(argv.contains("sleep infinity"), "{argv}");
        assert!(!argv.contains("--privileged"), "{argv}");
    }

    #[test]
    fn ensure_pulls_and_runs_missing_container() {
        let fake = FakePodman::new("", "ctr-id\n", 0);
        let m = mgr(&fake);
        let spec = ContainerSpec::for_workspace(Path::new("/repo/ws"));
        assert_eq!(m.ensure_running(&spec), ContainerState::Running);
        let calls = fake.calls();
        assert!(
            calls.contains("pull docker.io/library/ubuntu:24.04"),
            "{calls}"
        );
        assert!(calls.contains("run -d --name"), "{calls}");
    }

    #[test]
    fn ensure_leaves_existing_container_alone() {
        let spec = ContainerSpec::for_workspace(Path::new("/repo/ws"));
        let fake = FakePodman::new(&format!("{}\n", spec.name), "", 0);
        let m = mgr(&fake);
        assert_eq!(m.ensure_running(&spec), ContainerState::Running);
        let calls = fake.calls();
        assert!(!calls.contains("pull"), "{calls}");
        assert!(!calls.contains("run -d"), "{calls}");
    }

    #[test]
    fn cli_failure_lands_in_error() {
        let fake = FakePodman::new("", "boom\n", 1);
        let m = mgr(&fake);
        let spec = ContainerSpec::for_workspace(Path::new("/repo/ws"));
        assert!(matches!(m.ensure_running(&spec), ContainerState::Error(_)));
    }

    #[test]
    fn stop_missing_is_false_and_present_removes() {
        let spec = ContainerSpec::for_workspace(Path::new("/repo/ws"));
        let gone = FakePodman::new("", "", 0);
        assert!(!mgr(&gone).stop(&spec.name).unwrap());
        let here = FakePodman::new(&format!("{}\n", spec.name), "", 0);
        assert!(mgr(&here).stop(&spec.name).unwrap());
        assert!(here.calls().contains("rm -f"), "{}", here.calls());
    }
}
