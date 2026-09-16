//! Swappable agent socket (M3b, moved out of base): the agent
//! never knows Host vs VM, only tasks. `MockAgent` keeps CI green
//! without a server; `OpencodeAdapter` translates the opencode mirror
//! (`Store`) into the unified `AgentEvent` stream. `Supervisor` (M4c)
//! owns one backend plus one workspace face per task.
//!
//! Depends on `base` (Store + transcript types) and `workspace`
//! (Task + provider face); neither points back here.

pub mod agent;
pub mod driver;
pub mod supervisor;
pub mod sweeper;
pub mod terminal;

pub use agent::{
    AgentBackend, AgentEvent, AgentStatus, MockAgent, OpencodeAdapter, agent_status_of,
    apply_agent_event,
};
pub use driver::open_host_task;
pub use supervisor::Supervisor;
pub use sweeper::FleetSweeper;
pub use terminal::TerminalAdapter;
