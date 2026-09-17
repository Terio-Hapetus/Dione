//! M9b usage probe: CodexBar-style local-log parsing (no login, no network).
//!
//! Reads the JSONL logs CLIs already write (Claude Code, Codex CLI) into
//! token observations for [`base::MetricsLog`]. Defensive by design:
//! unknown lines/versions are skipped, a missing dir yields no samples —
//! never a crash (same contract as `load_agents_toml`). Dollars are never
//! guessed here; cost stays `None` until a priced source reports it.

use std::path::{Path, PathBuf};

use base::{TaskId, UsageSample};

/// One token observation from a local log line.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LogUsage {
    pub input: f64,
    pub output: f64,
    pub cache: f64,
    pub ts: u64,
}

impl LogUsage {
    pub fn tokens(&self) -> f64 {
        self.input + self.output + self.cache
    }
}

/// Token fill of a trailing quota window (CodexBar 5h/weekly cadence).
/// Percent is only reported when a plan `limit` is configured (M9d);
/// without it the UI shows raw window tokens, never a guessed %.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuotaWindow {
    pub window_secs: u64,
    pub tokens: f64,
    pub samples: u64,
    pub limit: Option<f64>,
}

impl QuotaWindow {
    pub fn of(usages: &[LogUsage], window_secs: u64, now: u64, limit: Option<f64>) -> Self {
        let mut tokens = 0.0;
        let mut samples = 0;
        for u in usages {
            if now.saturating_sub(u.ts) > window_secs {
                continue;
            }
            tokens += u.tokens();
            samples += 1;
        }
        Self {
            window_secs,
            tokens,
            samples,
            limit,
        }
    }

    pub fn used_pct(&self) -> Option<f64> {
        let limit = self.limit.filter(|l| *l > 0.0)?;
        Some(self.tokens / limit * 100.0)
    }
}

/// Default log locations. `None` = no home dir (tests pass explicit paths).
pub fn claude_log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

pub fn codex_log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
}

/// Recursively scan `dir` for `*.jsonl`, parse each, concatenate.
/// Missing dir, unreadable/oversize files → skipped silently.
/// Caps guard against runaway histories (CodexBar caps retention too).
pub fn scan_jsonl_logs(dir: &Path, parse: fn(&str) -> Vec<LogUsage>) -> Vec<LogUsage> {
    const MAX_FILES: usize = 1024;
    const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut files = 0;
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "jsonl") {
                if files >= MAX_FILES {
                    return out;
                }
                files += 1;
                let big = std::fs::metadata(&path)
                    .map(|m| m.len() > MAX_FILE_BYTES)
                    .unwrap_or(true);
                if big {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.extend(parse(&text));
                }
            }
        }
    }
    out
}

/// Attribute observations to one task for the metrics log (cost unknown).
pub fn samples_for(
    task: TaskId,
    agent: &str,
    model: &str,
    usages: &[LogUsage],
) -> Vec<UsageSample> {
    usages
        .iter()
        .map(|u| UsageSample {
            task,
            agent: agent.to_string(),
            model: model.to_string(),
            input: u.input,
            output: u.output,
            cache: u.cache,
            cost: None,
            ts: u.ts,
        })
        .collect()
}

/// Claude Code `~/.claude/projects/**/*.jsonl`: usage rides on
/// `message.usage` of assistant lines; user/summary lines carry none.
pub fn parse_claude_jsonl(text: &str) -> Vec<LogUsage> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(usage) = v.pointer("/message/usage") else {
            continue;
        };
        let Some(ts) = v.get("timestamp").and_then(ts_of) else {
            continue;
        };
        out.push(LogUsage {
            input: fnum(&usage["input_tokens"]),
            output: fnum(&usage["output_tokens"]),
            cache: fnum(&usage["cache_read_input_tokens"])
                + fnum(&usage["cache_creation_input_tokens"]),
            ts,
        });
    }
    out
}

/// Codex CLI `~/.codex/sessions/**/*.jsonl` (rollouts): token lines are
/// `{"type":"token_count","info":{"total_token_usage":{...}}}`;
/// every other line kind is skipped (formats drift across versions).
pub fn parse_codex_jsonl(text: &str) -> Vec<LogUsage> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        let Some(usage) = v.pointer("/info/total_token_usage") else {
            continue;
        };
        let Some(ts) = v.get("timestamp").and_then(ts_of) else {
            continue;
        };
        out.push(LogUsage {
            input: fnum(&usage["input_tokens"]),
            output: fnum(&usage["output_tokens"]),
            cache: fnum(&usage["cached_input_tokens"]),
            ts,
        });
    }
    out
}

fn fnum(v: &serde_json::Value) -> f64 {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_u64().map(|n| n as f64))
        .unwrap_or(0.0)
}

/// Log timestamps: unix seconds, unix millis, or ISO-8601 strings.
fn ts_of(v: &serde_json::Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        // Millis since forever ago; seconds fit far below the cut.
        return Some(if n >= 1_000_000_000_000 { n / 1000 } else { n });
    }
    if let Some(n) = v.as_i64() {
        let n = u64::try_from(n).ok()?;
        return Some(if n >= 1_000_000_000_000 { n / 1000 } else { n });
    }
    v.as_str().and_then(parse_iso8601)
}

/// Minimal ISO-8601 (`YYYY-MM-DDTHH:MM:SS[.frac][Z|±HH:MM]`). Anything
/// else → `None` (the line is skipped, not the whole file).
fn parse_iso8601(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let m: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() {
        return None;
    }
    // Split off the timezone tail first so fractions can't hide it.
    let (clock, tz) = match rest.find(['Z', '+', '-']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let mut c = clock.split(':');
    let hh: i64 = c.next()?.parse().ok()?;
    let mm: i64 = c.next()?.parse().ok()?;
    let ss: i64 = c.next()?.split('.').next()?.parse().ok()?;
    if c.next().is_some() {
        return None;
    }
    let off: i64 = match tz {
        "" | "Z" => 0,
        t => {
            let neg = t.starts_with('-');
            let t = &t[1..];
            let (oh, om) = t.split_once(':')?;
            let v: i64 = oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60;
            if neg { -v } else { v }
        }
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    if hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some((days_from_civil(y, m, day) * 86400 + hh * 3600 + mm * 60 + ss - off) as u64)
}

/// Howard Hinnant's civil→days; proleptic Gregorian, valid for log dates.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;
    use base::WINDOW_5H_SECS;

    const CLAUDE_LOG: &str = r#"
{"type":"user","message":{"role":"user","content":"hi"},"timestamp":"2026-09-01T10:00:00.000Z","uuid":"u1"}
{"type":"assistant","message":{"role":"assistant","content":[],"usage":{"input_tokens":5,"cache_creation_input_tokens":1,"cache_read_input_tokens":100,"output_tokens":20}},"timestamp":"2026-09-01T10:00:01Z","uuid":"a1"}
{"type":"summary","summary":"done","timestamp":"2026-09-01T10:00:02Z"}
not json at all
{"type":"assistant","message":{"role":"assistant"},"timestamp":"2026-09-01T10:00:03Z"}
"#;

    const CODEX_LOG: &str = r#"
{"timestamp":"2026-09-01T10:00:00Z","type":"session_meta","payload":{"id":"s"}}
{"timestamp":"2026-09-01T10:00:05Z","type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5},"last_token_usage":{"input_tokens":10,"output_tokens":5}}}
{"timestamp":1756720809000,"type":"token_count","info":{"total_token_usage":{"input_tokens":3,"output_tokens":1}}}
{"timestamp":"2026-09-01T10:00:07Z","type":"response_item","payload":{}}
"#;

    #[test]
    fn claude_parses_usage_and_skips_the_rest() {
        let got = parse_claude_jsonl(CLAUDE_LOG);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0],
            LogUsage {
                input: 5.0,
                output: 20.0,
                cache: 101.0,
                ts: 1788256801,
            }
        );
    }

    #[test]
    fn codex_parses_token_counts_both_ts_shapes() {
        let got = parse_codex_jsonl(CODEX_LOG);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].ts, 1788256805);
        assert_eq!(got[0].cache, 2.0);
        // Millis collapse to seconds.
        assert_eq!(got[1].ts, 1756720809);
        assert_eq!(got[1].cache, 0.0);
    }

    #[test]
    fn iso8601_offsets_and_rejects() {
        // +07:00 wall clock = Z minus 7h.
        assert_eq!(
            parse_iso8601("2026-09-01T17:00:01+07:00"),
            parse_iso8601("2026-09-01T10:00:01Z")
        );
        assert_eq!(parse_iso8601("2026-09-01 10:00:01"), None);
        assert_eq!(parse_iso8601("2026-13-01T10:00:01Z"), None);
        assert_eq!(parse_iso8601("2026-09-01T25:00:01Z"), None);
        assert_eq!(parse_iso8601("garbage"), None);
    }

    #[test]
    fn scan_walks_tree_and_ignores_missing() {
        let root = std::env::temp_dir().join(format!("dione-probe-{}", std::process::id()));
        let sub = root.join("proj").join("sess");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.jsonl"), CLAUDE_LOG).unwrap();
        std::fs::write(root.join("notes.txt"), "not a log").unwrap();
        let got = scan_jsonl_logs(&root, parse_claude_jsonl);
        assert_eq!(got.len(), 1);
        // Missing dir is empty, never an error.
        assert!(scan_jsonl_logs(&root.join("nope"), parse_claude_jsonl).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quota_window_fills_and_gates_pct_on_limit() {
        let usages = vec![
            LogUsage {
                input: 90.0,
                output: 10.0,
                cache: 0.0,
                ts: 900,
            },
            LogUsage {
                input: 1.0,
                output: 0.0,
                cache: 0.0,
                ts: 100,
            },
        ];
        let now = 1000;
        let w = QuotaWindow::of(&usages, WINDOW_5H_SECS, now, None);
        assert_eq!(w.tokens, 101.0);
        assert_eq!(w.samples, 2);
        assert_eq!(w.used_pct(), None);
        let w = QuotaWindow::of(&usages, 200, now, Some(200.0));
        assert_eq!(w.tokens, 100.0);
        assert_eq!(w.used_pct(), Some(50.0));
    }

    #[test]
    fn samples_attribute_without_dollars() {
        let task = TaskId::new();
        let got = samples_for(
            task,
            "claude",
            "sonnet",
            &[LogUsage {
                input: 1.0,
                output: 2.0,
                cache: 3.0,
                ts: 7,
            }],
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].task, task);
        assert_eq!(got[0].agent, "claude");
        assert_eq!(got[0].cost, None);
        assert_eq!(got[0].tokens(), 6.0);
    }
}
