//! M3a transcript: agent-agnostic mirror (`UnifiedMessage`/`Cost`).
//!
//! Legacy `Store` (`sessions`/`messages: Message/Part`) stays frozen;
//! this module adds the unified types every agent backend translates to.
//! UI v2 reads only `transcripts`/`costs`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use opencode_codes::protocol_generated::types::{Message, Part, ToolState};

/// Task identity. Full `Task` struct lands in M4; M3 only needs the id.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct TaskId(pub uuid::Uuid);

impl TaskId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TaskId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(uuid::Uuid::parse_str(s)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Agent,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub input_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnifiedMessage {
    pub id: String,
    pub task: TaskId,
    pub role: Role,
    pub text: String,
    pub tool: Option<ToolCall>,
    pub ts: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache: f64,
    pub cost: f64,
}

impl Cost {
    pub fn add(&mut self, o: Cost) {
        self.input += o.input;
        self.output += o.output;
        self.cache += o.cache;
        self.cost += o.cost;
    }
}

/// Cost carried by one legacy message, if any.
pub fn cost_of_message(info: &Message) -> Option<Cost> {
    match info {
        Message::Assistant(a) => Some(Cost {
            input: a.tokens.input,
            output: a.tokens.output,
            cache: a.tokens.cache.read,
            cost: a.cost,
        }),
        _ => None,
    }
}

/// Translate one legacy `(info, parts)` entry into unified messages.
/// One `User` message -> single `Role::User`; assistant parts each become
/// one message (`Text`/`Reasoning` -> `Agent`, `Tool` -> `Tool`).
pub fn entry_to_unified(task: TaskId, info: &Message, parts: &[Part]) -> Vec<UnifiedMessage> {
    match info {
        Message::User(u) => {
            let text = parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            vec![UnifiedMessage {
                id: u.id.clone(),
                task,
                role: Role::User,
                text,
                tool: None,
                ts: u.time.created as u64,
            }]
        }
        Message::Assistant(a) => parts
            .iter()
            .filter_map(|p| part_to_unified(task, a.time.created, p))
            .collect(),
    }
}

fn part_to_unified(task: TaskId, ts: u64, part: &Part) -> Option<UnifiedMessage> {
    match part {
        Part::Text(t) => Some(UnifiedMessage {
            id: t.id.clone(),
            task,
            role: Role::Agent,
            text: t.text.clone(),
            tool: None,
            ts,
        }),
        Part::Reasoning(r) => Some(UnifiedMessage {
            id: r.id.clone(),
            task,
            role: Role::Agent,
            text: r.text.clone(),
            tool: None,
            ts,
        }),
        Part::Tool(t) => {
            let (input_summary, text) = match &t.state {
                ToolState::Pending(s) => (summarize_input(&s.input), format!("⏳ {}", t.tool)),
                ToolState::Running(s) => (summarize_input(&s.input), format!("🔧 {}", t.tool)),
                ToolState::Completed(s) => (
                    summarize_input(&s.input),
                    if s.output.is_empty() {
                        format!("🔧 {}", s.title)
                    } else {
                        format!("🔧 {}\n{}", s.title, s.output.trim())
                    },
                ),
                ToolState::Error(s) => (
                    summarize_input(&s.input),
                    format!("❌ {}: {}", t.tool, s.error),
                ),
            };
            Some(UnifiedMessage {
                id: t.id.clone(),
                task,
                role: Role::Tool,
                text,
                tool: Some(ToolCall {
                    name: t.tool.clone(),
                    input_summary,
                }),
                ts,
            })
        }
        _ => None,
    }
}

fn summarize_input(input: &serde_json::Map<String, serde_json::Value>) -> String {
    let s = serde_json::to_string(input).unwrap_or_default();
    if s.chars().count() <= 200 {
        s
    } else {
        s.chars().take(200).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencode_codes::protocol_generated::types::{
        TextPart, TextPartInputTime, UserMessage, UserMessageModel, UserMessageTime,
    };

    fn user_info(id: &str) -> Message {
        Message::User(UserMessage {
            agent: "opencode".into(),
            format: None,
            id: id.into(),
            model: UserMessageModel {
                model_id: "m".into(),
                provider_id: "p".into(),
                variant: None,
            },
            role: "user".into(),
            session_id: "s".into(),
            summary: None,
            system: None,
            time: UserMessageTime { created: 42.0 },
            tools: None,
        })
    }

    fn text_part(id: &str, msg: &str, text: &str) -> Part {
        Part::Text(TextPart {
            id: id.into(),
            ignored: None,
            message_id: msg.into(),
            metadata: None,
            session_id: "s".into(),
            synthetic: None,
            text: text.into(),
            time: Some(TextPartInputTime {
                end: None,
                start: 1,
            }),
            type_: "text".into(),
        })
    }

    #[test]
    fn user_entry_joins_text_parts() {
        let task = TaskId::new();
        let info = user_info("u1");
        let parts = vec![
            text_part("p1", "u1", "hello"),
            text_part("p2", "u1", "world"),
        ];
        let out = entry_to_unified(task, &info, &parts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[0].text, "hello\nworld");
        assert_eq!(out[0].ts, 42);
    }

    #[test]
    fn task_id_roundtrips_display() {
        let id = TaskId::new();
        assert_eq!(id.to_string().parse::<TaskId>().unwrap(), id);
    }

    #[test]
    fn cost_accumulates() {
        let mut c = Cost::default();
        c.add(Cost {
            input: 1.0,
            output: 2.0,
            cache: 3.0,
            cost: 0.5,
        });
        c.add(Cost {
            input: 1.0,
            output: 0.0,
            cache: 0.0,
            cost: 0.5,
        });
        assert_eq!(c.input, 2.0);
        assert_eq!(c.cost, 1.0);
    }
}
