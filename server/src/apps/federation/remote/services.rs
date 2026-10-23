//! Service functions for remote.
//!
//! Adapt application use cases for views, commands, and worker scopes.

pub mod management;

pub mod runtime;

pub(crate) mod input_fences;

pub(crate) mod inbound;

pub(crate) mod local;

pub(crate) mod wire;
