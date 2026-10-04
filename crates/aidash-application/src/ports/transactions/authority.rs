//! Control scopes retain the same policy, credential and visibility locks until finish.
use crate::{Result, ports::transactions::TransactionAuthorityScope};
use aidash_domain::transactions::{
	Manifest,
	authority::{Binding, Origin, Preflight, Status},
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait ControlScope: Send {
	fn authority(&mut self) -> &mut dyn TransactionAuthorityScope;
	fn audit(&mut self, enabled: bool);
	async fn gate(&mut self) -> Result<()>;
	async fn status(&mut self, id: Uuid) -> Result<Status>;
	async fn match_origin(&mut self, id: Uuid, origin: &Origin) -> Result<()>;
	async fn bind(&mut self, id: Uuid, binding: &Binding) -> Result<()>;
	async fn trusted(&mut self, node: &str) -> Result<()>;
	async fn insert_attempt(&mut self, id: Uuid, node: &str) -> Result<()>;
	async fn pending_attempt(&mut self, id: Uuid, node: &str) -> Result<bool>;
	/// Transfer the existing transaction without releasing its authority locks.
	fn into_submission(self: Box<Self>) -> Result<Box<dyn SubmissionScope>>;
	/// Success commits; failure rolls back. Denials retain the adapter's audit behavior.
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}

#[async_trait]
pub trait SubmissionScope: Send {
	async fn submit(&mut self, manifest: &Manifest, origin: &Origin) -> Result<Status>;
	async fn bind(&mut self, id: Uuid, binding: &Binding) -> Result<()>;
	async fn abort(&mut self, id: Uuid) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}

#[async_trait]
pub trait AuthorityRepository: Send + Sync {
	fn node_id(&self) -> &str;
	fn validate(&self, manifest: &Manifest) -> Result<()>;
	async fn origin(&self, id: Uuid) -> Result<Option<Origin>>;
	async fn binding(&self, id: Uuid) -> Result<Option<Binding>>;
	async fn source(&self, origin: &Origin) -> Result<Box<dyn ControlScope>>;
	async fn mapped(&self, input: &Preflight) -> Result<Box<dyn ControlScope>>;
	async fn remote_preflight(&self, node: &str, input: &Preflight) -> Result<()>;
	async fn remote_access(&self, node: &str, input: &Preflight) -> Result<()>;
	async fn remote_ticket(&self, node: &str, id: Uuid) -> Result<Preflight>;
	async fn fault(&self, id: Uuid, point: &str) -> Result<()>;
	fn wake(&self);
}
