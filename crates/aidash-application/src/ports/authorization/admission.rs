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
	fn bundle(&self) -> &PolicyBundle;
	async fn task(&mut self, id: Uuid) -> Result<Option<Task>>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn require_live_agent(&mut self, task: Uuid, agent: &EntityRef) -> Result<()>;
	async fn executable_entry(&mut self, agent: &EntityRef) -> Result<Entry>;
	async fn active_installation(&mut self, entry: &Entry) -> Result<bool>;
	async fn check_pinned_installation(&mut self, entry: &Entry) -> Result<()>;
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
	) -> Result<Task>;
	async fn claimed_run(&mut self, task: Uuid) -> Result<Uuid>;
	async fn persist_grant(&mut self, grant: &ExecutionGrant) -> Result<()>;
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
