//! Deleting the Subnodes of a disabled creating Agent (#1012, ADR 0143:
//! "disabling or removing the creating Agent deletes its Subnodes").
//!
//! After a Catalog Approval is withdrawn or disabled, or the Tenant's policy
//! is saved, the change has committed: [`sweep`] then finds the active
//! Subnodes whose creating Agent version is definitely unusable on their Node
//! ([`aidash_domain::subnodes::creator_removed`]) and ends them the way their
//! creator's own end would, one creating Run per transaction. Each
//! transaction checks again under lock, so a version approved again in the
//! meantime keeps its Subnodes.
use crate::{
	Result,
	apps::{
		execution::repositories::subnodes::Cascade,
		identity::models::{AuthorizationBundle, AuthorizationCatalog},
	},
	authorization::Snapshot,
	registry::EntityRef,
	store::Store,
};
use aidash_application::{
	ports::subnodes::{CreatorRemovalScope, EndScope},
	tenancy::TenancyScope,
};
use aidash_domain::{
	ControlSource, RunControlAction,
	nodes::{DeploymentId, NodeAddress, NodeName},
	subnodes::{EndReason, SubnodeId},
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use uuid::Uuid;

/// Ends what a committed change made unusable. It holds none of the change's
/// locks. A failure is logged, not returned: the change itself stands, and the
/// next withdrawal or policy save in the Tenant sweeps again.
pub(crate) async fn sweep(store: &Store, tenant: &str) {
	match end_removed_creators(store, tenant).await {
		Ok(ended) if !ended.is_empty() => {
			tracing::info!(
				tenant,
				creators = ?ended,
				"ended the Subnodes of disabled creating Agents"
			);
		}
		Ok(_) => {}
		Err(error) => {
			tracing::warn!(
				%error,
				tenant,
				"could not look for the Subnodes of disabled creating Agents"
			);
		}
	}
}

/// The creating Runs whose Subnodes ended. One creating Run that fails is
/// logged and does not keep the others' Subnodes running.
pub(crate) async fn end_removed_creators(store: &Store, tenant: &str) -> Result<Vec<Uuid>> {
	let scope = TenancyScope::tenant(tenant)?;
	let candidates = store
		.pool
		.atomic(&scope, async |tx| {
			candidates(tx, &store.deployment, tenant).await
		})
		.await?;
	let mut ended = vec![];
	for creator in candidates {
		match end_one(store, &scope, tenant, creator).await {
			Ok(true) => ended.push(creator),
			Ok(false) => {}
			Err(error) => tracing::warn!(
				%error,
				tenant,
				%creator,
				"could not end the Subnodes of a disabled creating Agent"
			),
		}
	}
	Ok(ended)
}

async fn end_one(store: &Store, scope: &TenancyScope, tenant: &str, creator: Uuid) -> Result<bool> {
	let mut tx = store.pool.begin(scope).await?.into_executor().await?;
	let ended = aidash_application::subnodes::end_if_creator_removed(
		&mut Removal {
			cascade: Cascade {
				store,
				tx: tx.as_mut(),
			},
			tenant,
			policy: None,
		},
		creator,
	)
	.await;
	match ended {
		Ok(true) => {
			tx.commit().await?;
			Ok(true)
		}
		Ok(false) => {
			tx.rollback().await?;
			Ok(false)
		}
		Err(error) => {
			if let Err(rollback) = tx.rollback().await {
				tracing::debug!(%rollback, "creator removal rollback failed");
			}
			Err(error.into())
		}
	}
}

/// One active Subnode's creator, as the candidate read sees it.
struct Creator {
	run: Uuid,
	agent: EntityRef,
	node: NodeAddress,
}

/// The creating Runs of active Subnodes whose Agent version looks removed.
/// Only the policy row is share-locked, for this short read; each candidate is
/// decided again under lock.
async fn candidates(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	tenant: &str,
) -> Result<Vec<Uuid>> {
	let creators = active_creators(tx, deployment, tenant, None).await?;
	if creators.is_empty() {
		return Ok(vec![]);
	}
	let policy = AuthorizationBundle::lock_frozen(tx, tenant, false).await?;
	let mut removed = std::collections::BTreeSet::new();
	for creator in creators {
		let node = creator.node.to_string();
		let catalog = catalog_enabled(tx, tenant, &node, &creator.agent, false).await?;
		let subject = subject_enabled(&policy, &node, &creator.agent);
		if aidash_domain::subnodes::creator_removed(catalog, subject) {
			removed.insert(creator.run);
		}
	}
	Ok(removed.into_iter().collect())
}

/// The distinct creators of the Tenant's active Subnodes, or of one creating
/// Run's. Every Subnode of one creating Run has that Run's Agent and Node.
async fn active_creators(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	tenant: &str,
	creator: Option<Uuid>,
) -> Result<Vec<Creator>> {
	let mut query = Query::select();
	query
		.distinct()
		.columns(
			[
				"creator_run_id",
				"creator_agent_id",
				"creator_agent_version",
				"node",
			]
			.map(Alias::new),
		)
		.from(Alias::new("subnodes"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("state").eq("active"))
		.order_by(Alias::new("creator_run_id"), Order::Asc);
	if let Some(creator) = creator {
		query.and_where(Expr::col("creator_run_id").eq(Expr::value(creator)));
	}
	let rows: Vec<(Uuid, String, String, String)> =
		crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
			.columns(&[
				"creator_run_id",
				"creator_agent_id",
				"creator_agent_version",
				"node",
			])
			.fetch_all(&mut *tx)
			.await?;
	rows.into_iter()
		.map(|(run, id, version, node)| {
			Ok(Creator {
				run,
				agent: EntityRef { id, version },
				node: node_address(deployment, &node)?,
			})
		})
		.collect()
}

/// A Subnode records the address of the Node it runs on; a bare Node name
/// (as a seeded record may hold) is a Node of this Deployment.
fn node_address(deployment: &DeploymentId, node: &str) -> Result<NodeAddress> {
	match NodeAddress::parse(node) {
		Ok(address) => Ok(address),
		Err(_) => Ok(deployment.node(NodeName::parse(node)?)),
	}
}

/// The version's Catalog Approval on the Node at `node`; `None` without one.
async fn catalog_enabled(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	agent: &EntityRef,
	lock: bool,
) -> Result<Option<bool>> {
	let address = NodeAddress::parse(node)?;
	AuthorizationCatalog::enabled_in(tx, tenant, address.node.as_str(), agent, lock).await
}

/// Whether the policy names the Agent's subject on that Node, and enables it.
fn subject_enabled(policy: &Snapshot, node: &str, agent: &EntityRef) -> Option<bool> {
	subject_named(
		policy,
		&aidash_domain::agent_subject(node, &agent.id, &agent.version),
	)
}

fn subject_named(policy: &Snapshot, subject: &str) -> Option<bool> {
	policy
		.bundle
		.subjects
		.get(subject)
		.map(|subject| subject.enabled)
}

/// One creating Run's end, in its own transaction.
struct Removal<'a> {
	cascade: Cascade<'a>,
	tenant: &'a str,
	/// The policy `lock_creator` holds a shared lock on.
	policy: Option<Snapshot>,
}

#[async_trait]
impl EndScope for Removal<'_> {
	async fn active_subnodes(
		&mut self,
		creator: Uuid,
	) -> aidash_application::Result<Vec<(SubnodeId, Uuid)>> {
		self.cascade.active_subnodes(creator).await
	}
	async fn mark_ended(
		&mut self,
		id: SubnodeId,
		thread: Uuid,
		reason: EndReason,
	) -> aidash_application::Result<()> {
		self.cascade.mark_ended(id, thread, reason).await
	}
	async fn member_runs(
		&mut self,
		thread: Uuid,
		resuming: bool,
	) -> aidash_application::Result<Vec<(Uuid, bool)>> {
		self.cascade.member_runs(thread, resuming).await
	}
	async fn control_run(
		&mut self,
		run: Uuid,
		action: RunControlAction,
		source: ControlSource,
	) -> aidash_application::Result<()> {
		self.cascade.control_run(run, action, source).await
	}
}

#[async_trait]
impl CreatorRemovalScope for Removal<'_> {
	async fn lock_creator(
		&mut self,
		creator: Uuid,
	) -> aidash_application::Result<Option<(EntityRef, String)>> {
		// The policy first, shared, as a worker holds it before its Run; then
		// the creating Run, its active Subnodes and their Runs.
		self.policy =
			Some(AuthorizationBundle::lock_frozen(self.cascade.tx, self.tenant, false).await?);
		crate::apps::execution::repositories::subnodes::lock_for_end(self.cascade.tx, creator)
			.await?;
		let deployment = self.cascade.store.deployment.clone();
		Ok(
			active_creators(self.cascade.tx, &deployment, self.tenant, Some(creator))
				.await?
				.into_iter()
				.next()
				.map(|creator| (creator.agent, creator.node.to_string())),
		)
	}
	async fn catalog_enabled(
		&mut self,
		node: &str,
		agent: &EntityRef,
	) -> aidash_application::Result<Option<bool>> {
		Ok(catalog_enabled(self.cascade.tx, self.tenant, node, agent, true).await?)
	}
	async fn subject_enabled(&mut self, subject: &str) -> aidash_application::Result<Option<bool>> {
		let policy = self.policy.as_ref().ok_or_else(|| {
			aidash_application::Error::Invalid("the creator's policy is not locked".into())
		})?;
		Ok(subject_named(policy, subject))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn policy(subjects: serde_json::Value) -> Snapshot {
		Snapshot {
			revision: 1,
			bundle: serde_json::from_value(
				serde_json::json!({"tenant":"acme","subjects":subjects,"policies":[]}),
			)
			.unwrap(),
		}
	}

	#[test]
	fn a_bare_node_name_is_a_node_of_this_deployment() {
		let deployment = DeploymentId::parse("aidash://local").unwrap();
		let address = "aidash://local/brave-otter-k3m9q2xa";
		assert_eq!(
			node_address(&deployment, address).unwrap().to_string(),
			address
		);
		assert_eq!(
			node_address(&deployment, "brave-otter-k3m9q2xa")
				.unwrap()
				.to_string(),
			address
		);
		assert!(node_address(&deployment, "not a node").is_err());
	}

	#[test]
	fn the_creator_subject_is_read_on_the_subnode_node() {
		let node = "aidash://local/brave-otter-k3m9q2xa";
		let agent = EntityRef {
			id: "creator".into(),
			version: "1.0.0".into(),
		};
		let subject = format!("{node}/agents/creator@1.0.0");
		let disabled =
			policy(serde_json::json!({ subject.clone(): {"kind":"agent","enabled":false} }));
		assert_eq!(subject_enabled(&disabled, node, &agent), Some(false));
		let enabled = policy(serde_json::json!({ subject: {"kind":"agent"} }));
		assert_eq!(subject_enabled(&enabled, node, &agent), Some(true));
		// Another Node's subject for the same version says nothing here.
		let elsewhere = policy(serde_json::json!({
			"aidash://local/calm-heron-a1b2c3d4/agents/creator@1.0.0": {"kind":"agent","enabled":false}
		}));
		assert_eq!(subject_enabled(&elsewhere, node, &agent), None);
	}
}
