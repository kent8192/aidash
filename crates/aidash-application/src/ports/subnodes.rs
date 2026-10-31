//! Transactional capabilities for creating a Subnode.
use crate::{Result, ports::authorization::lease::AuthorizationLease, subnodes::Create};
use aidash_domain::{
	RunMetadata,
	policy::Resource,
	registry::EntityRef,
	subnodes::{Allowance, Subnode, SubnodeLimits},
};
use async_trait::async_trait;
use uuid::Uuid;

/// The implementation holds the creator, policy, limits and membership locks
/// until the resulting Workspace and Subnode are committed together. Counts
/// and idempotency must be serialized across concurrent calls by the same Run.
#[async_trait]
pub trait CreateScope: AuthorizationLease {
	/// The receiving Node address, independent of the Run's remote Home.
	fn execution_node(&self) -> &str;
	async fn node_resource(&mut self, run: &RunMetadata) -> Result<Resource>;
	async fn workspace_is_subnode(&mut self, workspace: Uuid) -> Result<bool>;
	async fn remote_grant_started_run(&mut self, run: Uuid) -> Result<bool>;
	/// Return an earlier receipt, or conflict if the key names different input.
	async fn replay(
		&mut self,
		run: &RunMetadata,
		key: &str,
		input: &Create,
	) -> Result<Option<Subnode>>;
	async fn limits(&mut self) -> Result<SubnodeLimits>;
	async fn active_count(&mut self, creator_run: Uuid) -> Result<usize>;
	/// The creator's bound tools and Secrets that the current lease may use.
	async fn creator_authority(&mut self, run: &RunMetadata) -> Result<Allowance>;
	/// Checks Node Catalog Approval and returns the member's complete Binding closure.
	async fn approved_member_authority(&mut self, agent: &EntityRef) -> Result<Allowance>;
	/// Creates the Workspace and membership atomically, with a durable receipt for `key`.
	async fn insert(&mut self, run: &RunMetadata, key: &str, input: &Create) -> Result<Subnode>;
}

/// All methods operate under the same transaction and lock the Subnode row.
/// Member changes and task admission must retain that lock until commit.
#[async_trait]
pub trait ManageScope: Send {
	async fn lock(&mut self, id: aidash_domain::subnodes::SubnodeId) -> Result<Option<Subnode>>;
	async fn limits(&mut self, tenant: &str) -> Result<SubnodeLimits>;
	/// Current creator authority; it may have narrowed since creation.
	async fn creator_authority(&mut self, subnode: &Subnode) -> Result<Allowance>;
	/// Checks Node Catalog Approval and the complete candidate Binding closure.
	async fn approved_member_authority(
		&mut self,
		subnode: &Subnode,
		agent: &EntityRef,
	) -> Result<Allowance>;
	async fn member_has_active_work(
		&mut self,
		subnode: &Subnode,
		agent: &EntityRef,
	) -> Result<bool>;
	async fn add_member(
		&mut self,
		subnode: &Subnode,
		agent: &EntityRef,
		caller: Uuid,
	) -> Result<()>;
	async fn remove_member(&mut self, subnode: &Subnode, agent: &EntityRef) -> Result<()>;
	/// Inserts the task in the Subnode Workspace and admits its leader with the
	/// creating Run's subject chain in the same transaction. Reuses `key`.
	async fn task(
		&mut self,
		subnode: &Subnode,
		run: &RunMetadata,
		key: &str,
		title: &str,
		request: &str,
	) -> Result<Uuid>;
	/// Marks dissolved and controls active member Runs using normal cancellation.
	async fn dissolve(
		&mut self,
		subnode: &Subnode,
		reason: aidash_domain::subnodes::EndReason,
	) -> Result<()>;
	/// Locks and checks the creator's current terminal state before insertion.
	async fn message_creator(
		&mut self,
		subnode: &Subnode,
		run: &RunMetadata,
		key: &str,
		content: &str,
	) -> Result<()>;
}

/// The creator Run is already locked; returned Subnodes and member Runs stay
/// locked until every control effect commits.
#[async_trait]
pub trait EndScope: Send {
	async fn active_subnodes(
		&mut self,
		creator: Uuid,
	) -> Result<Vec<(aidash_domain::subnodes::SubnodeId, Uuid)>>;
	async fn mark_ended(
		&mut self,
		id: aidash_domain::subnodes::SubnodeId,
		workspace: Uuid,
		reason: aidash_domain::subnodes::EndReason,
	) -> Result<()>;
	async fn active_runs(&mut self, workspace: Uuid) -> Result<Vec<Uuid>>;
	async fn control_run(
		&mut self,
		run: Uuid,
		action: aidash_domain::RunControlAction,
		source: aidash_domain::ControlSource,
	) -> Result<()>;
}
