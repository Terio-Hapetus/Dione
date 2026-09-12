use opencode_codes::protocol_generated::types::PermissionReplyResponse;

#[derive(Debug, Clone)]
pub enum Command {
    CreateSession {
        title: String,
    },
    SelectSession {
        id: String,
    },
    CreateWorktree {
        slug: String,
    },
    RemoveWorktree {
        slug: String,
    },
    MergeWorktree {
        slug: String,
    },
    SelectWorktree {
        slug: String,
    },
    Prompt {
        text: String,
    },
    FanOut {
        text: String,
    },
    /// Manual retry (M7b): reset the blocked task owning `slug` and
    /// re-track it with its own budget.
    RetryTask {
        slug: String,
    },
    Abort,
    FetchDiff(String),
    FetchAllDiffs,
    /// Cherry-pick hunks (M8a): apply selected unified-diff hunks of
    /// `file` into the main repo checkout.
    ApplyHunks {
        file: String,
        hunks: Vec<crate::worktree::Hunk>,
    },
    SendNotes {
        session_id: String,
        notes: Vec<crate::state::DiffNote>,
    },
    PermissionReply {
        permission_id: String,
        response: PermissionResponse,
    },
    SetModel {
        provider_id: String,
        model_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionResponse {
    Once,
    Always,
    Reject,
}

impl PermissionResponse {
    pub fn as_wire(&self) -> PermissionReplyResponse {
        match self {
            Self::Once => PermissionReplyResponse::Once,
            Self::Always => PermissionReplyResponse::Always,
            Self::Reject => PermissionReplyResponse::Reject,
        }
    }
}
