//! Publishing and live checks borrow the caller's existing authority transaction.
use crate::{Result, authorization::Snapshot};
use aidash_domain::{
	generation::requests::Request,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[async_trait]
pub trait GenerationPublication: Send {
	fn node_id(&self) -> &str;
	fn snapshot(&self) -> &Snapshot;
	fn replace_snapshot(&mut self, snapshot: Snapshot);
	async fn save_authority(&mut self, job: &Request) -> Result<()>;
	async fn register(&mut self, entry: &Entry) -> Result<()>;
	async fn approve(&mut self, job: &Request, entry: &Entry) -> Result<()>;
	async fn catalog_history(&mut self, job: &Request, entry: &Entry, actor: &str) -> Result<()>;
	/// The subject that approved the generation request, if one did.
	async fn approver(&mut self, job: &Request) -> Result<Option<String>>;
	/// Whether `subject` still holds `catalog.approve` on the generated version
	/// under the current policy.
	async fn may_approve(&mut self, subject: &str, entry: &Entry) -> Result<bool>;
}
#[async_trait]
pub trait GenerationLive: Send {
	fn tenant(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	async fn jobs(&mut self, node: &str, agent: &EntityRef) -> Result<Vec<Request>>;
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool>;
}
