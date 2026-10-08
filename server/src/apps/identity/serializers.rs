//! Serializers module for authorization app (RESTful)

pub mod policies;

pub mod catalog;

pub mod identity;

pub mod oidc;

pub mod peer_admission;

pub mod peer_execution;

pub mod peer_reads;

pub mod peer;

pub mod remote;

pub mod services_policy;

pub mod contracts;

pub mod session;

pub(crate) mod openapi;

pub mod settings;

pub mod peer_graph;

pub mod remote_execution;

pub mod remote_execution_commands;

pub(crate) mod desktop;

pub mod provider_credentials;
