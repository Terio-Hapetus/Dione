//! Swappable agent socket (M3b, moved out of ade-core): the agent
//! never knows Host vs VM, only tasks. `MockAgent` keeps CI green
//! without a server; `OpencodeAdapter` translates the opencode mirror
//! (`Store`) into the unified `AgentEvent` stream.
//!
//! Depends one-way on `ade-core` (Store + transcript types).

pub mod agent;

pub use agent::{
    AgentBackend, AgentEvent, AgentStatus, MockAgent, OpencodeAdapter, agent_status_of,
    apply_agent_event,
};
