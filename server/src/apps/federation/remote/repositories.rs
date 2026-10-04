//! Native adapters for delegation use cases and their durable visibility lease.
use crate::apps::federation::remote::models::Delegation as DelegationRecord;
use aidash_application::{
	Result,
	ports::federation::{DelegationRetries, FederationRepository},
};
use aidash_domain::{
	Run, Task,
	federation::{Delegation, Peer},
	registry::{AgentPage, EntityRef, Entry, Search},
};
use async_trait::async_trait;

pub(crate) struct Repository {
	pub federation: crate::federation::Federation,
}
struct Retries {
	pending: Vec<Delegation>,
	_visibility: crate::transactions::gate::ReadLease,
}
impl DelegationRetries for Retries {
	fn pending(&self) -> &[Delegation] {
		&self.pending
	}
}
#[async_trait]
impl FederationRepository for Repository {
	async fn peers(&self) -> Result<Vec<Peer>> {
		self.federation.peers().await.map_err(Into::into)
	}
	async fn peer(&self, node: &str) -> Result<Peer> {
		self.federation.peer(node).await.map_err(Into::into)
	}
	async fn task(&self, id: uuid::Uuid) -> Result<Task> {
		self.federation.store.task(id).await.map_err(Into::into)
	}
	async fn require_legacy_workspace(&self, id: uuid::Uuid) -> Result<()> {
		self.federation
			.store
			.require_legacy_execution(id)
			.await
			.map_err(Into::into)
	}
	async fn require_legacy_agent(&self, agent: &EntityRef) -> Result<()> {
		self.federation
			.store
			.require_legacy_agent(&agent.id, &agent.version)
			.await
			.map_err(Into::into)
	}
	async fn agent(&self, agent: &EntityRef) -> Result<Entry> {
		self.federation
			.registry
			.get(&agent.id, &agent.version)
			.await
			.map_err(Into::into)
	}
	async fn agents(&self, search: &Search, offset: u64) -> Result<AgentPage> {
		self.federation
			.registry
			.legacy_agents(search, offset)
			.await
			.map_err(Into::into)
	}
	async fn reserve_delegation(
		&self,
		task: &Task,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		let lease = self.federation.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				use reinhardt::db::orm::Model;
				let scoped = crate::apps::identity::models::AuthorizationWorkspace::objects()
					.filter(
						crate::apps::identity::models::AuthorizationWorkspace::field_workspace_id()
							.eq(task.workspace_id),
					)
					.exists_with_db(tx)
					.await?;
				if scoped {
					return Err(crate::Error::Forbidden);
				}
				let (_, delegation) = DelegationRecord::reserve(
					tx,
					&self.federation.config.node_id,
					task,
					node,
					agent,
				)
				.await?;
				Ok(delegation)
			})
			.await
			.map_err(Into::into)
	}
	async fn accept_local_run(&self, task: &Task, agent: &EntityRef) -> Result<Run> {
		self.federation
			.store
			.accept_run(
				task,
				&self.federation.config.node_id,
				&agent.id,
				&agent.version,
			)
			.await
			.map_err(Into::into)
	}
	async fn mark_delivered(&self, task: uuid::Uuid) -> Result<()> {
		let lease = self.federation.store.orm_connection()?;
		DelegationRecord::mark_delivered(&mut lease.handle(), task)
			.await
			.map_err(Into::into)
	}
	async fn claim_retries(&self) -> Result<Box<dyn DelegationRetries>> {
		let visibility =
			crate::transactions::gate::ReadLease::begin(&self.federation.store).await?;
		let lease = self.federation.store.orm_connection()?;
		let pending = DelegationRecord::claim_retries(lease.handle()).await?;
		Ok(Box::new(Retries {
			pending,
			_visibility: visibility,
		}))
	}
}
