//! Regression coverage for the identity adapters.
//!
//! App-owned integration targets are registered explicitly in `server/Cargo.toml`.
//! Execute an individual target with `cargo test -p aidash-server --test <target>`.
//!
//! - `dashboard_oidc`
//! - `execution_authorization`
//! - `interaction_authorization`
//! - `policy`
//! - `resource_authorization`
//! - `peer_graph`
//! - `peer_graph_nodes`
//! - `peer_graph_review`
//! - `scoped_remote_execution`
//!
//! Component tests also live beside the native repositories and service adapters.
