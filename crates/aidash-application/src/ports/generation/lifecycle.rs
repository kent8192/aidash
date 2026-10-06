//! Lifecycle scopes retain budget, authority, retirement, history and outbox atomicity.
use crate::{Result, authorization::Snapshot};
use aidash_domain::{
	generation::requests::{Control, Request},
	identity::Principal,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait GenerationLifecycleScope: Send {
	fn node_id(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	async fn unused(&mut self, job: &Request) -> Result<(i64, i64, i64)>;
	async fn release_policy(
		&mut self,
		job: &Request,
		unused: i64,
		unused_calls: i64,
		unused_embeddings: i64,
	) -> Result<()>;
	async fn mark_quota_released(&mut self, job: &Request) -> Result<()>;
	async fn cancel_runs(&mut self, job: &Request) -> Result<()>;
	async fn authority(&mut self, tenant: &str) -> Result<Snapshot>;
	async fn save_authority(
		&mut self,
		job: &Request,
		snapshot: &Snapshot,
		actor: &str,
	) -> Result<()>;
	async fn retire_catalog(&mut self, job: &Request) -> Result<Option<i64>>;
	async fn record_retirement(&mut self, job: &Request, revision: i64, actor: &str) -> Result<()>;
	async fn update_status(&mut self, job: &Request, status: &str) -> Result<Request>;
	async fn history(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
	async fn replay(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		input: &Control,
	) -> Result<bool>;
	async fn load(&mut self, tenant: &str, id: Uuid) -> Result<Request>;
}
#[async_trait]
pub trait GenerationControlScope: GenerationLifecycleScope {
	// This adapter invokes the shared application visibility use case against
	// the same owned subject scope; it must not create another transaction.
	async fn visible(&mut self, job: &Request) -> Result<bool>;
	async fn decide(&mut self, job: &Request, action: &str) -> Result<bool>;
	async fn finish(self: Box<Self>, result: Result<Request>) -> Result<Request>;
}
#[async_trait]
pub trait GenerationControls: Send + Sync {
	fn principal(&self) -> &Principal;
	async fn begin(&self, tenant: &str) -> Result<Box<dyn GenerationControlScope>>;
	fn notify(&self);
}
