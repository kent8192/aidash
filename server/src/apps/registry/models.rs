//! Persistent records owned by the registry app.

mod agent_knowledge;
pub use agent_knowledge::AgentKnowledge;
mod agent_plugin_copies;
pub use agent_plugin_copies::AgentPluginCopy;
mod registry;
pub use registry::Definition;
mod registry_agent_model_refs;
pub use registry_agent_model_refs::RegistryAgentModelRef;
mod registry_agent_resource_refs;
pub use registry_agent_resource_refs::RegistryAgentResourceRef;
mod registry_requests;
pub use registry_requests::RegistryRequest;

pub(crate) mod records;
pub mod states;
pub(crate) mod transaction_records;
