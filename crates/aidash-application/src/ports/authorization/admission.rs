//! Scoped admission keeps task/run/grant/delegation effects in one authority transaction.
use crate::{Result, ports::authorization::ExecutionGrantSession};
use aidash_domain::{
	Task,
	federation::Delegation,
	identity::execution::ExecutionGrant,
	policy::{PolicyBundle, Resource},
	registry::{AgentConfig, EntityRef, Entry},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait ExecutionAdmissionSession: ExecutionGrantSession {
	fn node_id(&self) -> &str;
	/// The address of the Node the Workspace belongs to, which executes its Tasks.
	async fn workspace_node(
		&mut self,
		workspace: Uuid,
	) -> Result<aidash_domain::nodes::NodeAddress>;
	fn bundle(&self) -> &PolicyBundle;
	async fn task(&mut self, id: Uuid) -> Result<Option<Task>>;
	/// Verify Subnode membership and task origin under the membership lock.
	async fn subnode_admission(&mut self, task: &Task, agent: &EntityRef) -> Result<()>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn require_live_agent(&mut self, task: Uuid, agent: &EntityRef) -> Result<()>;
	async fn executable_entry(&mut self, agent: &EntityRef) -> Result<Entry>;
	/// A pinned Decider's current entry, once the Run's subject may invoke it on
	/// this Node (`decision.invoke`): its Catalog Approval.
	async fn decider_entry(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn active_installation(&mut self, entry: &Entry) -> Result<bool>;
	async fn check_pinned_installation(&mut self, entry: &Entry) -> Result<()>;
	async fn bindings(
		&mut self,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let _ = entry;
		Err(crate::Error::Invalid(
			"Binding admission adapter is required".into(),
		))
	}
	async fn prepare_thread(
		&mut self,
		task: &Task,
		config: &AgentConfig,
		agent: &str,
	) -> Result<Option<Uuid>>;
	async fn claim(
		&mut self,
		task: &Task,
		revision: i64,
		subject: &str,
		entry: &Entry,
		snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	) -> Result<Task>;
	async fn claimed_run(&mut self, task: Uuid) -> Result<Uuid>;
	async fn persist_grant(&mut self, grant: &ExecutionGrant) -> Result<()>;
	/// Pins the Plan Connection of the grant's delegation-chain root for every
	/// Plan Connection model the snapshot selects (ADR 0007). `federated` is
	/// set when the task inherited a remote origin; such a Run never uses one.
	async fn admit_plan_connections(
		&mut self,
		grant: &ExecutionGrant,
		snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
		federated: bool,
	) -> Result<()>;
	async fn dashboard_origin(&mut self, credential: Uuid) -> Result<Option<(Uuid, Uuid)>>;
	async fn persist_dashboard_origin(
		&mut self,
		run: Uuid,
		identity: Uuid,
		mapping: Uuid,
	) -> Result<()>;
	async fn admit_thread(
		&mut self,
		task: &Task,
		run: Uuid,
		thread: Option<Uuid>,
		config: &AgentConfig,
		agent: &str,
	) -> Result<()>;
	async fn local_delegation(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation>;
	async fn delegation_event(&mut self, workspace: Uuid, delegation: &Delegation) -> Result<()>;
}
