//! Regression coverage for the identity adapters.
//!
//! App-owned integration targets are registered explicitly in `server/Cargo.toml`.
//! Execute an individual target with `cargo test -p aidash-server --test <target>`.
//!
//! - `dashboard_oidc`
//! - `demo_data`
//! - `execution_authorization`
//! - `interaction_authorization`
//! - `other_node_reads`
//! - `policy`
//! - `resource_authorization`
//! - `run_control`
//! - `approval_governs_use`
//! - `decision_counts`
//! - `peer_graph`
//! - `peer_graph_nodes`
//! - `peer_graph_review`
//! - `scoped_remote_execution`
//! - `secret_readiness`
//!
//! Component tests also live beside the native repositories and service adapters.
