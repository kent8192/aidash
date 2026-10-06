//! Provisioning sessions own the original authority locks and transaction boundaries.
use crate::{Result, ports::generation::publication::GenerationPublication};
use aidash_domain::{
	generation::{policy::Policy, requests::Request},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait GenerationActivationScope: GenerationPublication {
	fn now(&self) -> DateTime<Utc>;
	fn replace_subjects(&mut self, subjects: Vec<String>);
	async fn load(&mut self, tenant: &str, id: Uuid) -> Result<Request>;
	async fn visible(&mut self, job: &Request) -> Result<bool>;
	async fn require_request(&mut self, job: &Request) -> Result<()>;
	async fn current_policy(&mut self, job: &Request) -> Result<Policy>;
	async fn pinned_policy(&mut self, job: &Request) -> Result<Value>;
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	/// Invoke the lifecycle use case against this same authority transaction.
	async fn transition(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> Result<()>;
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<()>;
}

#[async_trait]
pub trait GenerationActivationSession: GenerationActivationScope {
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}

#[async_trait]
pub trait GenerationTerminalSession: Send {
	async fn load(&mut self, tenant: &str, id: Uuid) -> Result<Request>;
	async fn run_phase(&mut self, job: &Request) -> Result<Option<String>>;
	/// Invoke the lifecycle use case while retaining the exclusive authority lock.
	async fn transition(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}

#[async_trait]
pub trait GenerationProvisionRead: Send {
	/// Keep the node-wide visibility lease alive throughout this bounded scan.
	async fn jobs(&mut self) -> Result<Vec<Request>>;
	fn notify(&self);
}

#[async_trait]
pub trait GenerationProvisioning: Send + Sync {
	fn now(&self) -> DateTime<Utc>;
	async fn dispatch_remote(&self) -> Result<()>;
	async fn reconcile_foreign(&self) -> Result<()>;
	async fn begin_read(&self) -> Result<Box<dyn GenerationProvisionRead>>;
	async fn begin_activation(&self, job: &Request)
	-> Result<Box<dyn GenerationActivationSession>>;
	/// Drain explicitly opted-in completion work under the original live allowance.
	/// False defers natural retirement only; expiry and explicit controls still win.
	async fn completion_ready(&self, _job: &Request) -> Result<bool> {
		Ok(true)
	}
	/// Acquire the exclusive authority lock before reloading the request.
	async fn begin_terminal(&self, job: &Request) -> Result<Box<dyn GenerationTerminalSession>>;
}
