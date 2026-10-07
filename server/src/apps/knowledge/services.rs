//! Service functions for semantic.
//!
//! Adapt application use cases for views, commands, and inherited worker scopes.

pub use super::models::states;

pub mod entries;
pub(crate) mod memory_models;
pub mod memory_recovery;
pub mod native_memory;

pub mod core;
pub use core::*;

pub(crate) mod remote;

pub(crate) mod memory_context;

pub(crate) mod memory_administration;

pub(crate) mod remote_memory;
pub(crate) mod remote_memory_models;
