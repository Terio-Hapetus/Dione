//! Swappable agent socket (M3b, moved out of ade-core): the agent
//! never knows Host vs VM, only tasks. `MockAgent` keeps CI green
//! without a server; `OpencodeAdapter` translates the opencode mirror
//! (`Store`) into the unified `AgentEvent` stream. `Supervisor` (M4c)
//! owns one backend plus one workspace face per task.
//!
//! Depends on `ade-core` (Store + transcript types) and `ade-workspace`
//! (Task + provider face); neither points back here.

pub mod agent;
pub mod supervisor;

pub use agent::{
    AgentBackend, AgentEvent, AgentStatus, MockAgent, OpencodeAdapter, agent_status_of,
    apply_agent_event,
};
pub use supervisor::Supervisor;
