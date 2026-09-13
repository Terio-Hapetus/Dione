use opencode_codes::protocol_generated::types::{Message, MessageWithParts, Part};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnState {
    #[default]
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct MessageEntry {
    pub info: Message,
    pub parts: Vec<Part>,
}

impl From<MessageWithParts> for MessageEntry {
    fn from(m: MessageWithParts) -> Self {
        Self {
            info: m.info,
            parts: m.parts,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PendingPermission {
    pub permission_id: String,
    pub session_id: String,
    pub kind: String,
    pub patterns: Vec<String>,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderInfo {
    pub provider_id: String,
    pub provider_name: String,
    pub models: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct SelectedModel {
    pub provider_id: String,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffNote {
    pub session_id: String,
    pub file: String,
    pub line: u32,
    /// Multi-line range end (M8c): `None` (or `<= line`) = single line.
    pub end_line: Option<u32>,
    pub text: String,
    /// Thread replies (M8c2): follow-ups appended under the note.
    pub replies: Vec<String>,
}

/// Format review notes as a prompt body for the agent.
pub fn format_review_notes(notes: &[DiffNote]) -> String {
    let mut out = String::from("Review feedback — please address each item:\n");
    for n in notes {
        let loc = match n.end_line {
            Some(e) if e > n.line => format!("{}-{}", n.line, e),
            _ => format!("{}", n.line),
        };
        out.push_str(&format!("- {}:{} — {}\n", n.file, loc, n.text));
        for r in &n.replies {
            out.push_str(&format!("  ↳ {r}\n"));
        }
    }
    out
}

/// Append a reply to the note equal to `target` (M8c2, pure).
/// Returns false when the target is gone (deleted meanwhile).
pub fn append_reply(notes: &mut [DiffNote], target: &DiffNote, reply: String) -> bool {
    match notes.iter_mut().find(|n| *n == target) {
        Some(n) => {
            n.replies.push(reply);
            true
        }
        None => false,
    }
}

/// Anchor state machine for two-click range select (M8c, pure).
/// Returns `(start, end)` with `start <= end`, or `None` when the click
/// only sets/moves the anchor. Clicking the anchor line itself clears it
/// back to a single-line target.
pub fn resolve_range(
    anchor: Option<(String, String, u32)>,
    click: (String, String, u32),
) -> (Option<(String, String, u32)>, Option<(u32, u32)>) {
    let (sid, file, line) = click;
    match anchor {
        Some((a_sid, a_file, a_line)) if a_sid == sid && a_file == file => {
            if a_line == line {
                (None, Some((line, line)))
            } else {
                (None, Some((a_line.min(line), a_line.max(line))))
            }
        }
        _ => (Some((sid, file, line)), None),
    }
}

/// Split a combined unified diff into `(file, section)` pairs at
/// `diff --git` boundaries (pure). Preamble-less hunks yield no pairs;
/// callers surface those as one working-tree block instead.
pub fn split_files(raw: &str) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            out.push((file_from_git_header(rest), format!("{line}\n")));
        } else if let Some((_, section)) = out.last_mut() {
            section.push_str(line);
            section.push('\n');
        }
    }
    out
}

/// `a/foo b/foo` → `foo` (loose: strips quotes, takes the `b/` side).
fn file_from_git_header(rest: &str) -> Option<String> {
    let name = rest.rsplit(" b/").next()?.trim().trim_matches('"');
    (!name.is_empty()).then(|| name.to_string())
}

/// One rendered diff line with its new/old-file number, if countable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLine {
    /// 1-based line number (`+`/` ` lines: new file; `-` lines: old file).
    pub line: Option<u32>,
    pub text: String,
}

/// Map unified-diff lines to file line numbers. Handles multi-hunk patches;
/// `+++`/`---`/headers/no-newline markers get `None`.
pub fn parse_patch_lines(patch: &str) -> Vec<PatchLine> {
    let mut out = Vec::new();
    let mut old: u32 = 0;
    let mut new: u32 = 0;
    let mut in_hunk = false;
    for line in patch.lines() {
        if line.starts_with("@@") {
            for part in line.split_whitespace() {
                if let Some(v) = part.strip_prefix('-') {
                    old = v.split(',').next().unwrap_or("0").parse().unwrap_or(0);
                } else if let Some(v) = part.strip_prefix('+') {
                    new = v.split(',').next().unwrap_or("0").parse().unwrap_or(0);
                }
            }
            in_hunk = true;
            out.push(PatchLine {
                line: None,
                text: line.to_string(),
            });
        } else if !in_hunk || line.starts_with("+++") || line.starts_with("---") {
            out.push(PatchLine {
                line: None,
                text: line.to_string(),
            });
        } else if line.starts_with('+') {
            out.push(PatchLine {
                line: Some(new),
                text: line.to_string(),
            });
            new = new.saturating_add(1);
        } else if line.starts_with('-') {
            out.push(PatchLine {
                line: Some(old),
                text: line.to_string(),
            });
            old = old.saturating_add(1);
        } else if line.starts_with('\\') {
            out.push(PatchLine {
                line: None,
                text: line.to_string(),
            });
        } else {
            out.push(PatchLine {
                line: Some(new),
                text: line.to_string(),
            });
            old = old.saturating_add(1);
            new = new.saturating_add(1);
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct Totals {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cost: f64,
}

impl Totals {
    pub fn total_context(&self) -> f64 {
        self.input + self.cache_read
    }
}
