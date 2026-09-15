use std::path::PathBuf;

const CONFIG_DIR_NAME: &str = "ade";
const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub project_dir: PathBuf,
    pub opencode_binary: String,
    pub poll_interval_ms: u64,
    pub busy_poll_interval_ms: u64,
    /// Pinned theme (`"light"`/`"dark"`). `None` = follow the system.
    pub theme: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            project_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            opencode_binary: "opencode".to_string(),
            poll_interval_ms: 2_000,
            busy_poll_interval_ms: 600,
            theme: None,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME))
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    /// Load from an explicit path (pure-ish seam for tests; `load()`
    /// delegates with the real config dir).
    pub fn load_from(path: &std::path::Path) -> Self {
        let mut cfg = Self::default();
        let Ok(raw) = std::fs::read_to_string(path) else {
            return cfg;
        };
        let Ok(value) = raw.parse::<toml::Table>() else {
            return cfg;
        };
        if let Some(s) = value.get("project_dir").and_then(|v| v.as_str()) {
            cfg.project_dir = PathBuf::from(expand_home(s));
        }
        if let Some(s) = value.get("opencode_binary").and_then(|v| v.as_str()) {
            cfg.opencode_binary = s.to_string();
        }
        if let Some(n) = value.get("poll_interval_ms").and_then(|v| v.as_integer()) {
            cfg.poll_interval_ms = (n.max(200)) as u64;
        }
        if let Some(n) = value
            .get("busy_poll_interval_ms")
            .and_then(|v| v.as_integer())
        {
            cfg.busy_poll_interval_ms = (n.max(150)) as u64;
        }
        if let Some(s) = value.get("theme").and_then(|v| v.as_str())
            && (s == "light" || s == "dark")
        {
            cfg.theme = Some(s.to_string());
        }
        cfg
    }

    /// Persist the pinned theme (`None` = follow system again).
    /// Reads the existing table first so unrelated keys survive.
    pub fn save_theme(pref: Option<&str>) -> bool {
        let Some(path) = Self::config_path() else {
            return false;
        };
        Self::save_theme_to(&path, pref)
    }

    /// `save_theme` against an explicit path (tests; `save_theme`
    /// delegates with the real config dir).
    pub fn save_theme_to(path: &std::path::Path, pref: Option<&str>) -> bool {
        let mut table: toml::Table = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or_default();
        match pref {
            Some(t) => {
                table.insert("theme".to_string(), toml::Value::String(t.to_string()));
            }
            None => {
                table.remove("theme");
            }
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, table.to_string()).is_ok()
    }
}

fn expand_home(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ade-cfg-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ))
    }

    #[test]
    fn theme_round_trips_and_keeps_other_keys() {
        let dir = scratch("theme");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "opencode_binary = \"codex\"\n").unwrap();
        assert!(AppConfig::save_theme_to(&path, Some("light")));
        let cfg = AppConfig::load_from(&path);
        assert_eq!(cfg.theme.as_deref(), Some("light"));
        // Unrelated keys survive the rewrite.
        assert_eq!(cfg.opencode_binary, "codex");
        // Garbage theme values fall back to follow-system.
        std::fs::write(&path, "theme = \"neon\"\n").unwrap();
        assert_eq!(AppConfig::load_from(&path).theme, None);
        // Clearing the pin removes the key.
        assert!(AppConfig::save_theme_to(&path, None));
        assert_eq!(AppConfig::load_from(&path).theme, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_yields_follow_system_default() {
        let path = scratch("missing").join("config.toml");
        assert_eq!(AppConfig::load_from(&path).theme, None);
    }
}
