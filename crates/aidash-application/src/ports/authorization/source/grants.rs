//! Source grant scopes preserve current actor authority, shared leases and command serialization.
use crate::{
	Result,
	ports::authorization::{
		home::HomeScope,
		source::{SourceAuthorityScope, SourcePeerScope},
	},
	registry::DefinitionValidation,
};
use aidash_domain::{
	Task,
	federation::execution::{Inspection, PrepareInput, home::Grant},
	identity::execution::ExecutionPrincipal,
	semantic::remote::{Binding, Request},
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait GrantScope: HomeScope + SourceAuthorityScope + SourcePeerScope {
	fn replace_subjects(&mut self, subjects: Vec<String>);
	fn append_subject(&mut self, subject: String);
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<()>;
	async fn command_lock(&mut self, id: Uuid) -> Result<()>;
	async fn current_grant(&mut self, id: Uuid) -> Result<Grant>;
	async fn live(&mut self, id: Uuid) -> Result<bool>;
	async fn locked_task(&mut self, id: Uuid) -> Result<Task>;
	async fn grant_reads_visible(&mut self, id: Uuid) -> Result<bool>;
	async fn semantic_binding(
		&mut self,
		task: &Task,
		node: &str,
		inspection: &Inspection,
		request: &Request,
	) -> Result<Binding>;
	/// Preserve ON CONFLICT DO NOTHING and the database-clock TTL on the current transaction.
	async fn insert_grant(
		&mut self,
		input: &PrepareInput,
		task: &Task,
		metadata: &Value,
	) -> Result<u64>;
	async fn persist_semantic(&mut self, id: Uuid, semantic: &Value) -> Result<()>;
	async fn revocation_grant(&mut self, task: Uuid, id: Uuid) -> Result<Option<Grant>>;
	async fn revoke_locked(&mut self, id: Uuid) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
}
#[async_trait]
pub trait GrantRepository: Send + Sync {
	type Scope: GrantScope;
	fn source_node_id(&self) -> &str;
	fn source_identity(&self) -> Option<ExecutionPrincipal>;
	fn protocol_version(&self) -> &str;
	fn validation(&self) -> DefinitionValidation;
	async fn source_begin(&self) -> Result<Self::Scope>;
	async fn source_grant(&self, id: Uuid, node: &str) -> Result<Option<Grant>>;
	/// Saved authority deliberately reconstructs no browser session.
	async fn begin_grant(&self, grant: &Grant) -> Result<Self::Scope>;
	async fn source_request<T: DeserializeOwned + Send>(
		&self,
		node: &str,
		path: &str,
		input: &Value,
	) -> Result<T>;
}
