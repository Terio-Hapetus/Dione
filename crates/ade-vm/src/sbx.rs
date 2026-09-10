use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::backend::VmError;
use super::config::{SshInfo, VmConfig, VmHandle, VmState};

/// `sbx` CLI wrapper (Docker Sandboxes, each sandbox = 1 microVM).
///
/// Temporary scaffold: validates `VmManager` + `MicroVm` provider against
/// a real microVM early, then gets deleted in favor of CloudHypervisor
/// (sbx lacks the low-level controls we need: virtiofs tags, vsock CID,
/// image pinning). Assumed CLI syntax (pinned by fake-shim tests, must be
/// re-verified by the `ADE_LIVE_SBX=1` live test):
/// `create --name <n> shell <repo>`, `ls` table, `rm --force <n>`,
/// SSH via `<n>.sbx` after one `sbx setup ssh`.
#[derive(Debug)]
pub struct ExternalSbxBackend {
    bin: PathBuf,
    known: BTreeSet<String>,
    stopped: BTreeSet<String>,
    setup_ssh_done: AtomicBool,
    next_id: AtomicU64,
}

impl ExternalSbxBackend {
    pub fn new() -> Self {
        Self::with_bin(PathBuf::from("sbx"))
    }

    pub fn with_bin(bin: PathBuf) -> Self {
        Self {
            bin,
            known: BTreeSet::new(),
            stopped: BTreeSet::new(),
            setup_ssh_done: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        }
    }

    fn run(&self, args: &[&str]) -> anyhow::Result<std::process::Output> {
        let out = Command::new(&self.bin)
            .args(args)
            .output()
            .map_err(|e| anyhow::anyhow!("sbx spawn failed: {e:#}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "sbx {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out)
    }

    fn sandbox_name(&mut self, repo: &Path) -> String {
        let base: String = repo
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("ws")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_lowercase();
        let base = if base.is_empty() { "ws".into() } else { base };
        let mut name = format!("ade-{base}");
        if self.known.contains(&name) {
            let n = self.next_id.fetch_add(1, Ordering::SeqCst);
            name = format!("ade-{base}-{n}");
        }
        name
    }
}

impl super::backend::VmBackend for ExternalSbxBackend {
    fn boot(&mut self, cfg: &VmConfig) -> anyhow::Result<VmHandle> {
        let name = self.sandbox_name(&cfg.mount_repo);
        let repo = cfg.mount_repo.to_string_lossy().into_owned();
        // NOTE: vcpu/mem intentionally not passed: `sbx create` flag set
        // for cpus/memory is unverified; CloudHypervisor uses them fully.
        self.run(&["create", "--name", &name, "shell", &repo])?;
        self.known.insert(name.clone());
        self.stopped.remove(&name);
        Ok(VmHandle { id: name })
    }

    fn wait_ssh(&self, handle: &VmHandle) -> anyhow::Result<SshInfo> {
        if !self.known.contains(&handle.id) {
            return Err(VmError::UnknownHandle(handle.id.clone()).into());
        }
        // One probe attempt; VmManager polls this until its ssh timeout.
        // `sbx setup ssh` writes the managed `*.sbx` SSH-config block;
        // docs say it is safe to re-run, so a failed setup retries next poll.
        if !self.setup_ssh_done.load(Ordering::SeqCst) {
            let setup = Command::new(&self.bin)
                .args(["setup", "ssh"])
                .output()
                .map_err(|e| anyhow::anyhow!("sbx setup ssh failed: {e:#}"))?;
            if !setup.status.success() {
                anyhow::bail!(
                    "sbx setup ssh failed: {}",
                    String::from_utf8_lossy(&setup.stderr).trim()
                );
            }
            self.setup_ssh_done.store(true, Ordering::SeqCst);
        }
        let host = format!("{}.sbx", handle.id);
        let probe = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=2",
                "-o",
                "StrictHostKeyChecking=no",
                &host,
                "true",
            ])
            .output()
            .map_err(|e| anyhow::anyhow!("ssh probe failed: {e:#}"))?;
        if !probe.status.success() {
            anyhow::bail!("ssh not ready yet: {}", host);
        }
        Ok(SshInfo {
            host,
            port: 22,
            // Managed-block default; re-verify in the live test.
            user: "agent".into(),
        })
    }

    fn state_of(&self, handle: &VmHandle) -> VmState {
        if self.stopped.contains(&handle.id) {
            return VmState::Stopped;
        }
        if !self.known.contains(&handle.id) {
            return VmState::Missing;
        }
        let out = Command::new(&self.bin).arg("ls").output();
        let Ok(out) = out else {
            return VmState::Error("sbx ls spawn failed".into());
        };
        let text = String::from_utf8_lossy(&out.stdout);
        match parse_sbx_ls(&text).get(&handle.id) {
            Some(s) if s == "running" => VmState::Running,
            Some(s) if s == "stopped" || s == "exited" => VmState::Stopped,
            Some(_) => VmState::Booting,
            None => VmState::Booting,
        }
    }

    fn stop(&mut self, handle: &VmHandle) -> anyhow::Result<()> {
        if !self.known.contains(&handle.id) {
            return Err(VmError::UnknownHandle(handle.id.clone()).into());
        }
        if self.stopped.contains(&handle.id) {
            return Err(VmError::AlreadyStopped(handle.id.clone()).into());
        }
        self.run(&["rm", "--force", &handle.id])?;
        self.stopped.insert(handle.id.clone());
        Ok(())
    }
}

impl Default for ExternalSbxBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse `sbx ls` table output into sandbox-name → lowercase status.
/// Header-tolerant: skips the first line when it looks like a header.
pub(crate) fn parse_sbx_ls(output: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (i, line) in output.lines().enumerate() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            continue;
        }
        if i == 0 && cols[0].eq_ignore_ascii_case("sandbox") {
            continue;
        }
        map.insert(cols[0].to_string(), cols[2].to_lowercase());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VmBackend as _;

    #[test]
    fn parse_ls_table() {
        let out = "SANDBOX    AGENT  STATUS   PORTS  WORKSPACE\nade-api  shell  running  -  /home/u/api\nade-old  shell  stopped  -  /home/u/old\n";
        let m = parse_sbx_ls(out);
        assert_eq!(m.get("ade-api").map(String::as_str), Some("running"));
        assert_eq!(m.get("ade-old").map(String::as_str), Some("stopped"));
    }

    #[test]
    fn parse_ls_skips_garbage() {
        assert!(parse_sbx_ls("").is_empty());
        assert!(parse_sbx_ls("two words\n").is_empty());
        assert!(parse_sbx_ls("one\n").is_empty());
    }

    #[test]
    fn name_sanitizes_repo() {
        let mut b = ExternalSbxBackend::new();
        let n = b.sandbox_name(Path::new("/home/u/My_Proj.rs"));
        assert_eq!(n, "ade-my-proj-rs");
        b.known.insert(n.clone());
        // Collision → suffixed, never reused.
        assert_ne!(b.sandbox_name(Path::new("/home/u/My_Proj.rs")), n);
    }

    #[test]
    fn unknown_handle_states() {
        let b = ExternalSbxBackend::new();
        let h = VmHandle { id: "nope".into() };
        assert_eq!(b.state_of(&h), VmState::Missing);
        assert!(b.wait_ssh(&h).is_err());
    }
}
