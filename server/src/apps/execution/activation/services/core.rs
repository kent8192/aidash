//! Durable, node-scoped worker activation. PostgreSQL owns execution responsibility;
//! JetStream carries references, and never authorizes or determines Run behavior.
#[path = "broker.rs"]
pub(crate) mod broker;
#[path = "runtime.rs"]
pub(crate) mod runtime;
#[path = "settings.rs"]
pub(crate) mod settings;

pub use crate::apps::execution::activation::repositories::durable::request_in;
pub use aidash_domain::activation::Envelope;
pub use broker::Broker;
pub use runtime::Runtime;
pub use settings::Settings;
