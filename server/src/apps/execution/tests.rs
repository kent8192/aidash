//! Regression coverage for the execution adapters.
//!
//! App-owned integration targets are registered explicitly in `server/Cargo.toml`.
//! Execute an individual target with `cargo test -p aidash-server --test <target>`.
//!
//! - `commands`
//! - `endpoints`
//! - `event_bus`
//! - `framework_regressions`
//! - `generation`
//! - `generation_compaction`
//! - `generation_semantic`
//! - `inference_cancellation`
//! - `migrations`
//! - `observation`
//! - `postgres`
//! - `providers`
//! - `record_constraints`
//! - `reinhardt_persistence`
//! - `run_message_finalization`
//! - `startup`
//! - `streaming`
//! - `tool_batches`
//! - `core_capabilities`
//! - `http_protection`
//! - `multimodal_run`
//! - `worker_activation`
//! - `composite_keys`
//!
//! Component tests also live beside the native repositories and service adapters.

#[path = "tests/support/native_database.rs"]
pub(crate) mod native_database;
