//! inferd's interfaces and vocabulary: inference (`Inference`) and the agents' endpoints
//! (`Agents`), with the bus names, option keys, row keys and error prefixes that go with them, and
//! the codec of a need and a candidate. These leave porter with inferd. The AI keys, the agent
//! launcher's calls and the AI broker's questions are accountd's (`crate::accounts`).

mod agents;
mod codec;
mod failure;
mod interface;
mod local_engines;
mod names;
mod running;

use crate::Document;
use crate::Unserved;

pub use agents::{
    AGENT_ERROR_PREFIX, AgentsProxy, AgentsServed, AgentsServer, EndpointArg, MODELS_ANY,
    MODELS_LISTED, ROUTE_ACCOUNT, ROUTE_MODEL, RouteArg,
};
pub use codec::{NeedArg, need_from_dbus, need_to_dbus};
pub use failure::{PLACE_ERROR_PREFIX, computer_refusal_of, place_refusal_of};
pub use interface::{
    GuestAsks, GuestAsksStream, GuestsChanged, GuestsChangedStream, InferenceProxy,
    InferenceServed, InferenceServer,
};
pub use local_engines::{
    ENGINE_KEY_NAME, ENGINE_KEY_PATH, ENGINE_KEY_PLACE, ENGINE_KEY_SLOTS, ENGINE_KEY_STATE,
    ENGINE_KEY_VERSION, EnginePlace, EngineRowError, EngineState, LocalEngine,
    local_engine_from_dbus, local_engine_to_dbus,
};
pub use names::*;
pub use running::{
    RUNNING_KEY_ENGINE, RUNNING_KEY_MEMORY_MIB, RUNNING_KEY_REASON, RUNNING_KEY_SINCE,
    RUNNING_KEY_STATE, RUNNING_KEY_WINDOW, RunState, RunningModel, RunningRowError,
    running_from_dbus, running_to_dbus,
};

/// `org.quire.Inference1`.
pub const INFERENCE: Document = Document {
    file_name: "org.quire.Inference1.xml",
    interfaces: &[InferenceServed::<Unserved>::introspect],
};
/// `org.quire.Inference1.Agents`, on inferd's bus name.
pub const AGENTS: Document = Document {
    file_name: "org.quire.Inference1.Agents.xml",
    interfaces: &[AgentsServed::<Unserved>::introspect],
};
