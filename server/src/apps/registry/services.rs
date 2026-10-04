//! Service functions for registry.
//!
//! Adapt application use cases for views, commands, and worker scopes.

pub use super::models::states;

pub(crate) mod admission;

pub mod management;

pub mod core;
pub use core::*;
