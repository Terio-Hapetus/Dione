//! M1 store: worktrees (M0) + sessions/messages/permissions/diffs.
//! M2 links sessions to worktrees (`session_scope`) for the fleet dashboard.
//!
//! Facade: types live in `types.rs`, `Store` in `store.rs`,
//! SSE `apply_event` in `events.rs`. Public paths unchanged.

pub mod events;
pub mod store;
pub mod types;

pub use events::{apply_event, event_session_id, message_id};
pub use store::Store;
pub use types::{
    ConnState, DiffNote, MessageEntry, PatchLine, PendingPermission, ProviderInfo, SelectedModel,
    Totals, append_reply, format_review_notes, parse_patch_lines, resolve_range, split_files,
};
