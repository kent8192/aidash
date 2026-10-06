//! Durable agent execution and worker lifecycle over shared application ports.
//!
//! Native adapters supply persistence, authorization, inference and activation.
//! The harness owns agent steps, lease supervision, cancellation, recovery and
//! terminal delivery without depending on the server or process supervisor.
pub mod activation;
pub mod agent;
pub mod execution;
pub mod recovery;
