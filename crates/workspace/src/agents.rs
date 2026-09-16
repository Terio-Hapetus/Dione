//! M4b agents: `agents.toml` registry + `PATH` probe (Lab 4).
//!
//! The registry lives here — not in `agent` — because
//! `Task::agent_ref` resolves here and `PodmanProvider` execs the kit at boot.
//! A missing or unparseable file means an empty registry, never a crash.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One agent backend entry: `[agents.<name>] bin prompt_arg kit?`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    /// Binary to probe, e.g. `"claude"`, `"codex"`, `"opencode"`.
    pub bin: String,
    /// CLI flag carrying the prompt, e.g. `"-p"`.
    pub prompt_arg: String,
    /// Optional `kits/<name>.sh` installed at VM boot (ADR-0004).
    pub kit: Option<String>,
}

impl AgentEntry {
    pub fn new(bin: &str, prompt_arg: &str) -> Self {
        Self {
            bin: bin.to_string(),
            prompt_arg: prompt_arg.to_string(),
            kit: None,
        }
    }
}

/// `~/.config/ade/agents.toml` — same dir pattern as `AppConfig`.
pub fn default_agents_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ade").join("agents.toml"))
}

/// Parse the `[agents.<name>]` tables. Missing or invalid file → empty map.
pub fn load_agents_toml(path: &Path) -> BTreeMap<String, AgentEntry> {
    let mut out = BTreeMap::new();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(table) = raw.parse::<toml::Table>() else {
        return out;
    };
    let Some(agents) = table.get("agents").and_then(|v| v.as_table()) else {
        return out;
    };
    for (name, v) in agents {
        let Some(t) = v.as_table() else { continue };
        let bin = t
            .get("bin")
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .to_string();
        if bin.is_empty() {
            continue;
        }
        out.insert(
            name.clone(),
            AgentEntry {
                bin,
                prompt_arg: t
                    .get("prompt_arg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-p")
                    .to_string(),
                kit: t.get("kit").and_then(|v| v.as_str()).map(str::to_string),
            },
        );
    }
    out
}

/// Is `bin` runnable via `PATH` (or an explicit path)? No forking.
pub fn probe_bin(bin: &str) -> bool {
    if bin.is_empty() {
        return false;
    }
    let p = Path::new(bin);
    if p.components().count() > 1 {
        return is_executable(p);
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths)
        .map(|dir| dir.join(bin))
        .any(|p| is_executable(&p))
}

/// Probe every registry entry: name → binary present?
pub fn probe_all(reg: &BTreeMap<String, AgentEntry>) -> BTreeMap<String, bool> {
    reg.iter()
        .map(|(n, e)| (n.clone(), probe_bin(&e.bin)))
        .collect()
}

fn is_executable(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if md.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_toml(dir: &Path, body: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = dir.join(format!("agents-{}-{n}.toml", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    fn tmpdir() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("agents-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_full_and_partial_entries() {
        let d = tmpdir();
        let p = write_toml(
            &d,
            "[agents.claude]\nbin = \"claude\"\nprompt_arg = \"-p\"\nkit = \"kits/claude.sh\"\n\
             [agents.codex]\nbin = \"codex\"\n",
        );
        let reg = load_agents_toml(&p);
        assert_eq!(reg.len(), 2);
        assert_eq!(
            reg["claude"],
            AgentEntry {
                bin: "claude".into(),
                prompt_arg: "-p".into(),
                kit: Some("kits/claude.sh".into()),
            }
        );
        // prompt_arg defaults to -p, kit to None.
        assert_eq!(reg["codex"], AgentEntry::new("codex", "-p"));
    }

    #[test]
    fn missing_file_and_invalid_toml_yield_empty() {
        let d = tmpdir();
        assert!(load_agents_toml(&d.join("does-not-exist.toml")).is_empty());
        let bad = write_toml(&d, "[agents.oops\nbin = = =");
        assert!(load_agents_toml(&bad).is_empty());
        let nobin = write_toml(&d, "[agents.empty]\nbin = \"\"\n");
        assert!(load_agents_toml(&nobin).is_empty());
    }

    #[test]
    fn probe_finds_sh_and_rejects_missing() {
        assert!(probe_bin("sh"));
        assert!(!probe_bin("ade-no-such-bin-xyz"));
        assert!(!probe_bin(""));
    }

    #[test]
    fn probe_all_maps_registry() {
        let mut reg = BTreeMap::new();
        reg.insert("good".into(), AgentEntry::new("sh", "-p"));
        reg.insert("bad".into(), AgentEntry::new("ade-no-such-bin-xyz", "-p"));
        let got = probe_all(&reg);
        assert!(got["good"]);
        assert!(!got["bad"]);
    }
}
