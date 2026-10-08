//! Sandbox inference authority and durable observations reuse existing native transactions.
use super::dispatch::{RealDispatchRepository, RealDispatchScope};
use crate::Result;
use aidash_domain::registry::{
	Entry,
	workbench::{
		Draft,
		sandbox::{TestOutcome, TestSession},
	},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ExecutionScope: RealDispatchScope {
	/// Reuse the canonical draft-content/dependency authorization use case on this same scope.
	async fn bindings(
		&mut self,
		draft: &Draft,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot>;
	async fn validate_content(&mut self, draft: &Draft) -> Result<Entry>;
}
#[async_trait]
pub trait ExecutionRepository: RealDispatchRepository {
	async fn begin_execution(&self) -> Result<Box<dyn ExecutionScope + '_>>;
	/// Read and commit one observation before inference or outcome classification.
	async fn read_session(&self, id: Uuid) -> Result<TestSession>;
	/// Preserve the running-only predicate and atomic conversation/calls/usage update.
	async fn progress(
		&self,
		id: Uuid,
		conversation: Value,
		calls: Value,
		usage: Value,
	) -> Result<()>;
	/// Preserve the running-only predicate and release the active slot atomically.
	async fn finish(&self, id: Uuid, outcome: &TestOutcome) -> Result<()>;
}
