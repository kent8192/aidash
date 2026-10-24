//! Regression coverage for the registry adapters.
//!
//! App-owned integration targets are registered explicitly in `server/Cargo.toml`.
//! Execute an individual target with `cargo test -p aidash-server --test <target>`.
//!
//! - `agent_workbenches`
//! - `personal_agents`
//! - `provider_timeout_persistence`
//! - `registry_and_execution_contracts`
//! - `workbench_regressions`
//!
//! Component tests also live beside the native repositories and service adapters.
