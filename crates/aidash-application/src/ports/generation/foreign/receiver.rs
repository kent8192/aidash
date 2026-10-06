//! The execution peer prepares one foreign executor inside its mapped policy lease.
use crate::Result;
use aidash_domain::{
	generation::{
		intent::{Intent, Prepared, guards::Authority},
		policy::{Policy, Spec},
		requests::{Assignment, Request},
	},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait ForeignGenerationReceiverScope: Send {
	fn authority(&self) -> Authority;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn policy(&mut self, id: &str) -> Result<Policy>;
	/// Read the nonterminal Home/task request under the original update lock.
	async fn existing(&mut self, source: &str, task: Uuid) -> Result<Option<Request>>;
	async fn replayed(&mut self, source: &str, id: Uuid) -> Result<bool>;
	async fn create(&mut self, intent: &Intent, policy: Policy) -> Result<Assignment>;
	async fn publish(&mut self, job: &Request, spec: &Spec) -> Result<()>;
	/// Keep the conditional QUEUED update in the same transaction as publication.
	async fn mark_prepared(&mut self, id: Uuid) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<Prepared>) -> Result<Prepared>;
}
#[async_trait]
pub trait ForeignGenerationReceiver: Send + Sync {
	fn node_id(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	async fn describe(&self, source: &str, id: Uuid) -> Result<Intent>;
	async fn begin(
		&self,
		source: &str,
		tenant: &str,
		subject: &str,
	) -> Result<Box<dyn ForeignGenerationReceiverScope>>;
}
