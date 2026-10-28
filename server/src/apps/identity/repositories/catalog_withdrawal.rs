//! Withdrawal impact reads one Tenant's approved agents and in-flight Runs on
//! one Node under the requester's authorization transaction.
use crate::{
	apps::{
		execution::models::{Run, states::RunPhase},
		identity::models::AuthorizationCatalog,
	},
	authorization::access::Access,
	registry::{EntityRef, Entry},
};
use aidash_application::Result;
use aidash_application::ports::catalog_withdrawal::WithdrawalImpactScope;
use aidash_domain::nodes::NodeAddress;
use async_trait::async_trait;
use reinhardt::db::orm::Model;

pub(crate) struct WithdrawalScope<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) node: NodeAddress,
}

fn orm(error: reinhardt::core::exception::Error) -> aidash_application::Error {
	crate::Error::from(error).into()
}

#[async_trait]
impl WithdrawalImpactScope for WithdrawalScope<'_> {
	fn node(&self) -> &NodeAddress {
		&self.node
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Option<Entry>> {
		match crate::apps::registry::models::Definition::read_in(
			self.access.tx.native_mut(),
			&reference.id,
			&reference.version,
		)
		.await
		{
			Ok(entry) => Ok(Some(entry)),
			Err(crate::Error::NotFound(_)) => Ok(None),
			Err(error) => Err(error.into()),
		}
	}
	async fn can_approve(&mut self, entry: &Entry) -> Result<bool> {
		// The same `registry` resource `TenantCatalog::require` checks when the
		// approval is withdrawn.
		let resource = self.access.resource(
			"registry",
			format!("{}/{}@{}", entry.kind, entry.id, entry.version),
			aidash_application::authorization::catalog::attributes(entry),
		);
		self.access
			.decide(
				&resource,
				aidash_application::authorization::catalog::approvals::CATALOG_APPROVE,
			)
			.await
			.map_err(Into::into)
	}
	async fn approved_agents(&mut self) -> Result<Vec<Entry>> {
		let rows = AuthorizationCatalog::objects()
			.filter(AuthorizationCatalog::field_tenant().eq(self.access.identity.tenant.clone()))
			.filter(AuthorizationCatalog::field_node().eq(self.node.node.as_str().to_owned()))
			.filter(AuthorizationCatalog::field_enabled().eq(true))
			.order_by(&["entry_id", "entry_version"])
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(|error| orm(error.into()))?;
		let mut agents = Vec::new();
		for row in rows {
			let reference = EntityRef {
				id: row.entry_key,
				version: row.entry_version,
			};
			if let Some(entry) = self.definition(&reference).await?
				&& entry.kind == "agent"
			{
				agents.push(entry);
			}
		}
		Ok(agents)
	}
	async fn non_terminal_runs(&mut self, agents: &[EntityRef]) -> Result<u64> {
		let mut total = 0;
		for agent in agents {
			total += Run::objects()
				.filter(Run::field_tenant().eq(self.access.identity.tenant.clone()))
				.filter(Run::field_node().eq(self.node.node.as_str().to_owned()))
				.filter(Run::field_agent_id().eq(agent.id.clone()))
				.filter(Run::field_agent_version().eq(agent.version.clone()))
				.filter(Run::field_phase().is_in([
					RunPhase::Ready,
					RunPhase::Thinking,
					RunPhase::ToolCall,
					RunPhase::Waiting,
				]))
				.count_with_executor(self.access.tx.native_mut())
				.await
				.map_err(|error| orm(error.into()))? as u64;
		}
		Ok(total)
	}
}
