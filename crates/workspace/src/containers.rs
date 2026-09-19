//! P3 containers: lifecycle over the podman CLI (ADR-0006).
//!
//! One container per worktree (ADR-0007; ADR-0002 said per workspace):
//! the manager ensures a long-lived `sleep infinity` container with the
//! repo root bind-mounted at [`crate::podman::CTR_WORKSPACE`]. Agents then
//! run via `PodmanProvider` (`exec`/`shell`). Only communication is CLI
//! argv — no SSH, no vsock, no key material.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::provider::retry_busy;

/// Pinned guest image (digest-pin in production configs, tag here).
pub const CTR_IMAGE: &str = "docker.io/library/ubuntu:24.04";
/// Guest dev-server port published as `host 41xx` (preview convention).
pub const PREVIEW_CTR_PORT: u16 = 3000;

/// Lifecycle states. No `WaitingSsh`/`Mounting` — there is no guest
/// SSH or virtiofs mount to wait for (anti-legacy rule, ADR-0006).
/// `Paused` is ADR-0007 per-worktree sleep: `Running` → `pause` →
/// `Paused` → `unpause` → `Running` (cgroup freezer, keep mount).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Missing,
    Pulling,
    Running,
    Paused,
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
        let out = retry_busy(|| Command::new(&self.bin).args(args).output())
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

    /// Current state via `podman inspect` (`--format {{.State.Status}}`):
    /// `running` → `Running`, `paused` → `Paused`, `paused` is ADR-0007
    /// sleep; `created`/`exited` → `Stopped`; absent → `Missing`.
    /// Unknown raw strings surface as `Error` so new podman states don't
    /// silently pretend to be `Running`.
    pub fn inspect_state(&self, name: &str) -> anyhow::Result<ContainerState> {
        let out = retry_busy(|| {
            Command::new(&self.bin)
                .args(["inspect", "--format", "{{.State.Status}}", name])
                .output()
        })
        .map_err(|e| anyhow::anyhow!("podman spawn failed: {e:#}"))?;
        if !out.status.success() {
            // `inspect` exits non-zero when the container doesn't exist
            // (the `ps`-fallback path below would be racy for this caller).
            // Treat stderr "no such" as Missing, else bubble the error.
            let err = String::from_utf8_lossy(&out.stderr);
            if err.to_lowercase().contains("no such") {
                return Ok(ContainerState::Missing);
            }
            anyhow::bail!("podman inspect failed: {}", {
                let t = err.trim();
                if t.is_empty() { "inspect failed" } else { t }
            });
        }
        Ok(match String::from_utf8_lossy(&out.stdout).trim() {
            "running" => ContainerState::Running,
            "paused" => ContainerState::Paused,
            "created" | "exited" | "stopped" => ContainerState::Stopped,
            other => ContainerState::Error(format!("unknown state {other:?}")),
        })
    }

    /// Pause a running container (ADR-0007 sleep). No-op when missing or
    /// already paused; errors land in `Error` so callers surface them.
    pub fn pause(&self, name: &str) -> ContainerState {
        match self.inspect_state(name) {
            Ok(ContainerState::Missing) => ContainerState::Missing,
            Ok(ContainerState::Paused) => ContainerState::Paused,
            Ok(ContainerState::Running) => {
                match self.run_cli(&["pause".to_string(), name.to_string()]) {
                    Ok(_) => ContainerState::Paused,
                    Err(e) => ContainerState::Error(format!("pause failed: {e:#}")),
                }
            }
            Ok(other) => ContainerState::Error(format!("pause: not running (is {other:?})")),
            Err(e) => ContainerState::Error(format!("pause inspect: {e:#}")),
        }
    }

    /// Unpause (wake) a paused container. No-op when already running;
    /// reports `Missing` when the container doesn't exist.
    pub fn unpause(&self, name: &str) -> ContainerState {
        match self.inspect_state(name) {
            Ok(ContainerState::Missing) => ContainerState::Missing,
            Ok(ContainerState::Running) => ContainerState::Running,
            Ok(ContainerState::Paused) => {
                match self.run_cli(&["unpause".to_string(), name.to_string()]) {
                    Ok(_) => ContainerState::Running,
                    Err(e) => ContainerState::Error(format!("unpause failed: {e:#}")),
                }
            }
            Ok(other) => ContainerState::Error(format!("unpause: not paused (is {other:?})")),
            Err(e) => ContainerState::Error(format!("unpause inspect: {e:#}")),
        }
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
            Ok(_) => ContainerState::Running,
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

    /// ADR-0007: fake `inspect` returns `status` per subcommand type
    /// (`inspect` field + `ps` fallback), so pause/unpause state machine
    /// is testable without a real cgroup freezer.
    #[derive(Debug)]
    struct InspectPodman {
        _dir: PathBuf,
        bin: PathBuf,
    }

    impl InspectPodman {
        fn new(state: &str, code: i32) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(100_000);
            let dir = std::env::temp_dir().join(format!(
                "dione-inspect-podcli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("state.txt"), state).unwrap();
            std::fs::write(dir.join("code.txt"), code.to_string()).unwrap();
            let bin = dir.join("podman");
            let script = format!(
                "#!/bin/sh\necho \"$@\" >> \"{0}/calls.txt\"\nif [ \"$1\" = \"inspect\" ]; then cat \"{0}/state.txt\"; exit \"$(cat \"{0}/code.txt\")\"; fi\ncat \"{0}/state.txt\"\nexit 0\n",
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

    impl Drop for InspectPodman {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self._dir);
        }
    }

    #[test]
    fn pause_needs_running_and_unpause_needs_paused() {
        let running = InspectPodman::new("running\n", 0);
        let mgr = ContainerManager::new().with_bin(running.bin.clone());
        assert_eq!(mgr.pause("n"), ContainerState::Paused);
        assert!(running.calls().contains("pause"), "{}", running.calls());
        // Fresh `running` so the second `inspect` still returns running
        // (the `pause` simulation doesn't flip state.txt → unpause needs
        // its own fresh fake).
        let running2 = InspectPodman::new("running\n", 0);
        let mgr2 = ContainerManager::new().with_bin(running2.bin.clone());
        // `unpause` no-ops when already running; use a paused one to test it
        let paused = InspectPodman::new("paused\n", 0);
        assert_eq!(
            ContainerManager::new()
                .with_bin(paused.bin.clone())
                .unpause("n"),
            ContainerState::Running
        );
        assert!(paused.calls().contains("unpause"), "{}", paused.calls());
        let _ = running2;
        let _ = mgr2;
    }

    #[test]
    fn pause_paused_is_noop_unpause_running_is_noop() {
        let paused = InspectPodman::new("paused\n", 0);
        assert_eq!(
            ContainerManager::new()
                .with_bin(paused.bin.clone())
                .pause("n"),
            ContainerState::Paused
        );
        assert!(!paused.calls().contains("pause"), "{}", paused.calls());
        let running = InspectPodman::new("running\n", 0);
        assert_eq!(
            ContainerManager::new()
                .with_bin(running.bin.clone())
                .unpause("n"),
            ContainerState::Running
        );
        assert!(!running.calls().contains("unpause"), "{}", running.calls());
    }

    #[test]
    fn missing_and_wrong_state_surface_cleanly() {
        // `inspect` non-zero + "no such container" → Missing.
        let miss = InspectPodman::new("", 1);
        std::fs::write(miss._dir.join("state.txt"), "").unwrap();
        // Script exits 1 but stdout is empty; we need stderr "no such"
        // — rebuild with stderr branch for this case only.
        let bin = miss._dir.join("podman-miss");
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> \"{}/calls.txt\"\nif [ \"$1\" = \"inspect\" ]; then echo \"Error: no such container n\" >&2; exit 1; fi\nexit 0\n",
            miss._dir.display(),
        );
        std::fs::write(&bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mgr = ContainerManager::new().with_bin(bin);
        assert_eq!(mgr.inspect_state("n").unwrap(), ContainerState::Missing);
        assert_eq!(mgr.pause("n"), ContainerState::Missing);
        assert_eq!(mgr.unpause("n"), ContainerState::Missing);
        // Pausing a stopped container is a clean Error, not Missing.
        let stopped = InspectPodman::new("exited\n", 0);
        assert!(matches!(
            ContainerManager::new()
                .with_bin(stopped.bin.clone())
                .pause("n"),
            ContainerState::Error(_)
        ));
        assert!(matches!(
            ContainerManager::new()
                .with_bin(stopped.bin.clone())
                .unpause("n"),
            ContainerState::Error(_)
        ));
    }
}
