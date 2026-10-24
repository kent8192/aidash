//! Regression coverage for the federation adapters.
//!
//! App-owned integration targets are registered explicitly in `server/Cargo.toml`.
//! Execute an individual target with `cargo test -p aidash-server --test <target>`.
//!
//! - `outbound_discovery`
//! - `peer_authorization`
//! - `peer_execution`
//! - `remote_admission`
//! - `remote_grants`
//! - `remote_reads`
//! - `run_message_fence`
//! - `run_message_remote`
//! - `transaction_protocol`
//! - `transactions`
//! - `transaction_acceptance`
//!
//! Component tests also live beside the native repositories and service adapters.
