//! Service functions for semantic.
//!
//! Adapt application use cases for views, commands, and inherited worker scopes.

pub use super::models::states;

pub mod entries;

pub mod core;
pub use core::*;

pub(crate) mod remote;
