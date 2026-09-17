//! M9e secrets: BYOK keys live in the OS keychain, never on disk.
//!
//! Contract (ADR-0006 hardening): secrets travel only as `podman exec
//! -e K=V` flags (or host process env) at spawn time. They never enter
//! the container filesystem, logs, transcripts, or the UI (presence
//! renders as a glyph, values are never read back for display).
//!
//! Backend is the Secret Service CLI (`secret-tool`, libsecret): zero new
//! crates, same fake-binary test seam as `PodmanProvider`. Missing tool
//! or locked collection → readable error, never a crash (Host mode
//! keeps working; the agent just lacks that key).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::agents::AgentEntry;
use super::provider::retry_busy;

/// Keychain service name for every Dione secret.
pub const SECRET_SERVICE: &str = "dione";

/// OS-keychain face. Object-safe so supervisors hold it boxed.
/// `account` is `"<agent>/<ENV_VAR>"` (see [`secret_account`]).
pub trait Secrets: Send {
    fn get(&self, account: &str) -> anyhow::Result<Option<String>>;
    fn set(&mut self, account: &str, secret: &str) -> anyhow::Result<()>;
    fn delete(&mut self, account: &str) -> anyhow::Result<bool>;
}

/// Canonical account name: agent-scoped so two agents never share a key
/// entry by accident (`claude/ANTHROPIC_API_KEY`).
pub fn secret_account(agent: &str, key: &str) -> String {
    format!("{agent}/{key}")
}

/// In-memory secrets for tests, CI, and keychain-less machines.
#[derive(Debug, Default)]
pub struct MockSecrets {
    map: BTreeMap<String, String>,
}

impl MockSecrets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, account: &str, secret: &str) -> Self {
        self.map.insert(account.to_string(), secret.to_string());
        self
    }
}

impl Secrets for MockSecrets {
    fn get(&self, account: &str) -> anyhow::Result<Option<String>> {
        Ok(self.map.get(account).cloned())
    }

    fn set(&mut self, account: &str, secret: &str) -> anyhow::Result<()> {
        self.map.insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&mut self, account: &str) -> anyhow::Result<bool> {
        Ok(self.map.remove(account).is_some())
    }
}

/// Secret Service via the `secret-tool` CLI (libsecret). Test seam:
/// any `secret-tool`-compatible binary works (`with_bin`).
#[derive(Debug)]
pub struct CliSecrets {
    bin: PathBuf,
}

impl CliSecrets {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from("secret-tool"),
        }
    }

    /// Test seam: fake `secret-tool` binary.
    pub fn with_bin(mut self, bin: PathBuf) -> Self {
        self.bin = bin;
        self
    }

    fn lookup(&self, account: &str) -> anyhow::Result<Option<String>> {
        let out = retry_busy(|| {
            Command::new(&self.bin)
                .args(["lookup", "service", SECRET_SERVICE, "account", account])
                .output()
        })
        .map_err(|e| anyhow::anyhow!("secret-tool spawn failed: {e:#}"))?;
        if out.status.success() {
            return Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()));
        }
        // Exit 1 = no such item (libsecret convention); anything else
        // (locked collection, no D-Bus) is also "absent here", surfaced
        // by the caller counting `missing`.
        Ok(None)
    }
}

impl Default for CliSecrets {
    fn default() -> Self {
        Self::new()
    }
}

impl Secrets for CliSecrets {
    fn get(&self, account: &str) -> anyhow::Result<Option<String>> {
        self.lookup(account)
    }

    fn set(&mut self, account: &str, secret: &str) -> anyhow::Result<()> {
        let mut child = retry_busy(|| {
            Command::new(&self.bin)
                .args([
                    "store",
                    "--label",
                    "dione",
                    "service",
                    SECRET_SERVICE,
                    "account",
                    account,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
        })
        .map_err(|e| anyhow::anyhow!("secret-tool spawn failed: {e:#}"))?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("secret-tool stdin unavailable"))?
            .write_all(secret.as_bytes())?;
        let out = child.wait_with_output()?;
        anyhow::ensure!(
            out.status.success(),
            "secret-tool store failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }

    fn delete(&mut self, account: &str) -> anyhow::Result<bool> {
        let out = retry_busy(|| {
            Command::new(&self.bin)
                .args(["clear", "service", SECRET_SERVICE, "account", account])
                .output()
        })
        .map_err(|e| anyhow::anyhow!("secret-tool spawn failed: {e:#}"))?;
        Ok(out.status.success())
    }
}

/// Is a Secret Service CLI runnable via `PATH`? Missing → secrets fall
/// back to explicit env inheritance + a UI hint, never a crash.
pub fn probe_secret_tool() -> bool {
    crate::agents::probe_bin("secret-tool")
}

/// Resolved spawn env for one agent: present keys become `(K, V)` pairs
/// for `-e` flags / process env; absent keys are listed, never empty
/// strings (an empty key would silently mis-authenticate the agent).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResolvedEnv {
    pub vars: Vec<(String, String)>,
    pub missing: Vec<String>,
}

pub fn resolve_task_env(
    reg: &BTreeMap<String, AgentEntry>,
    secrets: &dyn Secrets,
    agent_ref: &str,
) -> ResolvedEnv {
    let mut out = ResolvedEnv::default();
    let Some(entry) = reg.get(agent_ref) else {
        return out;
    };
    for key in &entry.env_keys {
        match secrets.get(&secret_account(agent_ref, key)) {
            Ok(Some(v)) => out.vars.push((key.clone(), v)),
            _ => out.missing.push(key.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// Fake `secret-tool`: `lookup` prints canned secret (exit from
    /// code.txt), `store`/`clear` succeed. Records argv. Parallel-safe.
    struct FakeSecretTool {
        _dir: PathBuf,
        bin: PathBuf,
    }

    static NEXT_FAKE: AtomicU64 = AtomicU64::new(1);

    impl FakeSecretTool {
        fn new(secret: &str, found: bool) -> Self {
            use std::sync::atomic::Ordering;
            let dir = std::env::temp_dir().join(format!(
                "dione-fake-secret-{}-{}",
                std::process::id(),
                NEXT_FAKE.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("secret.txt"), secret).unwrap();
            let bin = dir.join("secret-tool");
            let script = format!(
                "#!/bin/sh\necho \"$@\" > \"{0}/args.txt\"\nif [ \"$1\" = \"lookup\" ]; then if [ -f \"{0}/missing\" ]; then exit 1; else cat \"{0}/secret.txt\"; exit 0; fi; fi\nexit 0\n",
                dir.display(),
            );
            std::fs::write(&bin, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let fake = Self { _dir: dir, bin };
            if !found {
                std::fs::write(fake._dir.join("missing"), "").unwrap();
            }
            fake
        }

        fn args(&self) -> String {
            std::fs::read_to_string(self._dir.join("args.txt")).unwrap()
        }
    }

    impl Drop for FakeSecretTool {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self._dir);
        }
    }

    #[test]
    fn accounts_are_agent_scoped() {
        assert_eq!(
            secret_account("claude", "ANTHROPIC_API_KEY"),
            "claude/ANTHROPIC_API_KEY"
        );
    }

    #[test]
    fn mock_round_trips() {
        let mut m = MockSecrets::new().with("claude/K", "v");
        assert_eq!(m.get("claude/K").unwrap(), Some("v".into()));
        assert_eq!(m.get("nope").unwrap(), None);
        m.set("n", "1").unwrap();
        assert!(m.delete("n").unwrap());
        assert!(!m.delete("n").unwrap());
    }

    #[test]
    fn cli_lookup_found_and_missing() {
        let fake = FakeSecretTool::new("s3cr3t\n", true);
        let cli = CliSecrets::new().with_bin(fake.bin.clone());
        assert_eq!(cli.get("claude/K").unwrap(), Some("s3cr3t\n".into()));
        assert!(
            fake.args()
                .contains("lookup service dione account claude/K")
        );
        let gone = FakeSecretTool::new("", false);
        let cli = CliSecrets::new().with_bin(gone.bin.clone());
        assert_eq!(cli.get("claude/K").unwrap(), None);
    }

    #[test]
    fn cli_store_and_clear_shape() {
        let fake = FakeSecretTool::new("", true);
        let mut cli = CliSecrets::new().with_bin(fake.bin.clone());
        cli.set("claude/K", "v").unwrap();
        assert!(fake.args().contains("store --label dione service dione"));
        assert!(cli.delete("claude/K").unwrap());
    }

    #[test]
    fn resolve_splits_present_and_missing() {
        use std::collections::BTreeMap;

        use super::super::agents::AgentEntry;

        let mut reg = BTreeMap::new();
        let mut entry = AgentEntry::new("claude", "-p");
        entry.env_keys = vec!["A".into(), "B".into()];
        reg.insert("claude".into(), entry);
        let secrets = MockSecrets::new().with("claude/A", "1");
        let got = resolve_task_env(&reg, &secrets, "claude");
        assert_eq!(got.vars, vec![("A".into(), "1".into())]);
        assert_eq!(got.missing, vec!["B".to_string()]);
        // Unknown agent: empty, never an error.
        assert_eq!(
            resolve_task_env(&reg, &secrets, "nope"),
            ResolvedEnv::default()
        );
    }

    #[test]
    fn probe_never_panics() {
        let _ = probe_secret_tool();
    }
}
