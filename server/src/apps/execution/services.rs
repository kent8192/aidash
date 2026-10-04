//! Service functions for harness.
//!
//! Adapt application use cases for views, commands, and worker scopes.

pub mod context_rules;
pub(crate) mod human_interaction;
pub(crate) mod input_ledger;

pub use super::models::states;

pub mod management;

pub mod bus;

pub mod capabilities;

pub mod openrouter;

pub mod provider;

pub mod runtime;

pub mod web_search;

pub mod context;

pub mod frontend;
pub mod lifecycle;
pub mod schema;
pub mod worker;

pub mod metrics;
