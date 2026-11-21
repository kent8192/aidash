//! Source grant scopes preserve current actor authority, shared leases and command serialization.
use crate::{
	Result,
	authorization::source::grants::{DependencyChecks, GrantPeerCache},
	ports::authorization::{
		home::HomeScope,
		source::{SourceAuthorityScope, SourcePeerScope},
	},
	registry::DefinitionValidation,
};
use aidash_domain::{
	Task,
	federation::{
		dependencies::Input,
		execution::{Inspection, PrepareInput, home::Grant},
		route::Route,
		tool_lists::FederationToolRequest,
	},
	identity::execution::ExecutionPrincipal,
	semantic::remote::{Binding, Request},
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
#[async_trait]
pub trait GrantScope: HomeScope + SourceAuthorityScope + SourcePeerScope {
	/// The address of the Home Node that issued the grant, known once the
	/// task's Thread has been read in this scope.
	fn home_address(&self) -> String;
	/// What the Home's Node Registration of `receiver` asks it to let this grant's
	/// Runs use (ADR 0139), read now. `None` when the Home registers no such Node,
	/// as for a receiver of the Home's own Tenant, whose Nodes need no registration.
	async fn registered_tool_request(
		&mut self,
		receiver: &str,
	) -> Result<Option<FederationToolRequest>>;
	/// The tools the Home's people allowed this grant's Run for that Run only (ADR 0139),
	/// merged into the Request its Description carries. None unless an ask was answered.
	async fn run_only_tool_request(
		&mut self,
		_grant: Uuid,
	) -> Result<aidash_domain::federation::tool_lists::ToolList> {
		Ok(Default::default())
	}
	fn replace_subjects(&mut self, subjects: Vec<String>);
	fn append_subject(&mut self, subject: String);
	/// Record a refusal after policy evaluation but before peer admission.
	async fn record_refusal(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		reason: &str,
	) -> Result<()>;
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<()>;
	/// A Subnode Thread may delegate only to its local members.
	async fn require_non_subnode_thread(&mut self, thread: Uuid) -> Result<()>;
	async fn command_lock(&mut self, id: Uuid) -> Result<()>;
	async fn current_grant(&mut self, id: Uuid) -> Result<Grant>;
	async fn live(&mut self, id: Uuid) -> Result<bool>;
	async fn locked_task(&mut self, id: Uuid) -> Result<Task>;
	/// Limit dependency traversal to evidence already fetched outside this scope.
	fn set_dependency_checks(&mut self, checks: Option<DependencyChecks>);
	/// Roll back a read-only pass and release its locks before contacting a peer.
	async fn release_probe(self) -> Result<()>;
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
	async fn event(&mut self, thread: Uuid, kind: &str, data: Value) -> Result<()>;
}
#[async_trait]
pub trait GrantRepository: Send + Sync {
	type Scope: GrantScope;
	fn peer_cache(&self) -> Arc<GrantPeerCache>;
	async fn check_dependency(
		&self,
		route: &Route,
		input: &Input,
		remaining: Duration,
	) -> crate::ports::federation::dependencies::Check;
	fn source_node_id(&self) -> &str;
	fn source_identity(&self) -> Option<ExecutionPrincipal>;
	fn validation(&self) -> DefinitionValidation;
	async fn source_begin(&self) -> Result<Self::Scope>;
	async fn source_grant(&self, id: Uuid, node: &str) -> Result<Option<Grant>>;
	/// Saved authority deliberately reconstructs no browser session.
	async fn begin_grant(&self, grant: &Grant) -> Result<Self::Scope>;
	async fn source_request<T: DeserializeOwned + Send>(
		&self,
		home: &str,
		node: &str,
		path: &str,
		input: &Value,
		identity: &ExecutionPrincipal,
	) -> Result<T>;
}
