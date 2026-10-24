//! A Tenant's Nodes: listing, creating within "Nodes per Tenant" and
//! relabeling (#241). Members list them; Tenant Administrators create and
//! relabel them, as do Deployment Administrators.
use crate::apps::identity::models::{AdministrationActor, AdministrationEvent, NodeDeletion};
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::repositories::{node_deletions, nodes, tenants::Creator};
use crate::apps::identity::serializers::nodes::{
	NodeCreation, NodeDeletionRecord, NodeDeletionRequest, NodeLabelUpdate, NodeList, NodeRecord,
	NodeState,
};
use crate::apps::identity::services::access::NativeAccess;
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::authorization::identity::Actor;
use crate::federation::Federation;
use crate::{Error, Result};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::tenant_administration as rules;
use aidash_domain::nodes::{self as domain, Node};
use aidash_domain::tenancy::TenantName;
use serde_json::json;

pub struct Nodes {
	runtime: Federation,
}

#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> Nodes {
	Nodes { runtime }
}

impl Nodes {
	fn administrations(&self) -> TenantAdministrations {
		TenantAdministrations {
			runtime: self.runtime.clone(),
		}
	}

	fn record(&self, node: Node, deletion: Option<NodeDeletion>) -> NodeRecord {
		let state = match &deletion {
			None => NodeState::Live,
			Some(row) if row.purged_at.is_some() => NodeState::Purged,
			Some(row) if row.purge_step.is_some() || row.purge_lease.is_some() => {
				NodeState::Deleting
			}
			Some(_) => NodeState::Deleted,
		};
		let storage_retained_until_purge =
			deletion.as_ref().is_some_and(|row| row.purged_at.is_none());
		NodeRecord {
			address: self
				.runtime
				.config
				.deployment
				.node(node.name.clone())
				.to_string(),
			name: node.name.into_string(),
			label: node.label,
			default: node.default,
			created_at: node.created_at,
			state,
			storage_retained_until_purge,
			retained_storage_bytes: None,
			deletion: deletion.map(|row| NodeDeletionRecord {
				id: row.id,
				deleted_at: row.deleted_at,
				deleted_by_kind: row.deleted_by_kind,
				purge_after: row.purge_after,
				purged_at: row.purged_at,
				purge_step: row.purge_step,
				purge_failures: row.purge_failures,
				purge_error: row.purge_error,
				purge_failed_at: row.purge_failed_at,
			}),
		}
	}

	async fn records_in(
		&self,
		tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
		tenant: &str,
		page: &NodeList,
	) -> Result<Vec<NodeRecord>> {
		let rows = if page.include_deleted {
			nodes::list_management(tx, tenant, page.limit, page.offset).await?
		} else {
			nodes::list_live(tx, tenant).await?
		};
		let mut deletions = if page.include_deleted {
			node_deletions::current_of(
				tx,
				tenant,
				rows.iter()
					.map(|node| node.name.as_str().to_owned())
					.collect(),
			)
			.await?
		} else {
			Default::default()
		};
		let mut records = Vec::with_capacity(rows.len());
		for node in rows {
			let bytes = if page.include_deleted {
				Some(
					super::node_purge::retained_storage_bytes_in(tx, tenant, node.name.as_str())
						.await?,
				)
			} else {
				None
			};
			let deletion = deletions.remove(node.name.as_str());
			let mut record = self.record(node, deletion);
			record.retained_storage_bytes = bytes;
			records.push(record);
		}
		Ok(records)
	}

	pub(crate) async fn list(
		&self,
		actor: Actor,
		tenant: String,
		page: NodeList,
	) -> Result<Vec<NodeRecord>> {
		crate::http::validate(&page)?;
		let f = &self.runtime;
		let rows = match actor {
			Actor::DeploymentAdministrator => {
				let scope = TenancyScope::tenant(&tenant)?;
				f.store
					.pool
					.atomic(&scope, async |tx| {
						super::remote::deployment_administrator::tenant(tx, &tenant).await?;
						self.records_in(tx, &tenant, &page).await
					})
					.await?
			}
			Actor::Subject(identity) => {
				if identity.tenant != tenant {
					return Err(Error::Forbidden);
				}
				let mut access = NativeAccess::begin(&f.store, &identity).await?;
				let result = async {
					let resource = access.resource("node", rules::COLLECTION, json!({}));
					access
						.require(
							&resource,
							if page.include_deleted {
								rules::NODE_DELETE
							} else {
								rules::NODE_READ
							},
						)
						.await?;
					let (tx, tenant) = access.tenant_transaction();
					self.records_in(tx, tenant, &page).await
				}
				.await;
				access.finish(result).await?
			}
		};
		Ok(rows)
	}

	pub(crate) async fn create(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: NodeCreation,
	) -> Result<NodeRecord> {
		let label = domain::label(&input.label)?;
		let f = self.runtime.clone();
		let node = match actor {
			Actor::DeploymentAdministrator => {
				let scope = TenancyScope::tenant(&tenant)?;
				f.store
					.pool
					.atomic(&scope, async |tx| {
						super::remote::deployment_administrator::tenant(tx, &tenant).await?;
						let actor = AdministrationActor::DeploymentAdministratorBearer;
						create_in(
							&f,
							tx,
							&tenant,
							&label,
							Creator::DeploymentAdministrator(None),
							&actor,
						)
						.await
					})
					.await?
			}
			actor => {
				self.administrations()
					.run(actor, origin, &tenant, async |administrator| {
						f.store
							.pool
							.atomic(&administrator.scope()?, async |tx| {
								let snapshot = administrator.lock(tx, false).await?;
								administrator
									.require(tx, &snapshot, rules::NODE_CREATE, &tenant, json!({}))
									.await?;
								let actor =
									AdministrationActor::TenantAdministrator(administrator.clone());
								create_in(
									&f,
									tx,
									&tenant,
									&label,
									Creator::Identity(administrator.identity_id),
									&actor,
								)
								.await
							})
							.await
					})
					.await?
			}
		};
		Ok(self.record(node, None))
	}

	pub(crate) async fn relabel(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		input: NodeLabelUpdate,
	) -> Result<NodeRecord> {
		let label = domain::label(&input.label)?;
		let name = domain::NodeName::parse(&node)?;
		let f = self.runtime.clone();
		let node = match actor {
			Actor::DeploymentAdministrator => {
				let scope = TenancyScope::tenant(&tenant)?;
				f.store
					.pool
					.atomic(&scope, async |tx| {
						super::remote::deployment_administrator::tenant(tx, &tenant).await?;
						relabel_in(
							tx,
							&tenant,
							name.as_str(),
							&label,
							&AdministrationActor::DeploymentAdministratorBearer,
						)
						.await
					})
					.await?
			}
			actor => {
				self.administrations()
					.run(actor, origin, &tenant, async |administrator| {
						f.store
							.pool
							.atomic(&administrator.scope()?, async |tx| {
								let snapshot = administrator.lock(tx, false).await?;
								let current = nodes::get_live(tx, &tenant, name.as_str()).await?;
								administrator
									.require(
										tx,
										&snapshot,
										rules::NODE_UPDATE,
										name.as_str(),
										json!({"label": current.label, "default": current.default}),
									)
									.await?;
								relabel_in(
									tx,
									&tenant,
									name.as_str(),
									&label,
									&AdministrationActor::TenantAdministrator(
										administrator.clone(),
									),
								)
								.await
							})
							.await
					})
					.await?
			}
		};
		Ok(self.record(node, None))
	}

	pub(crate) async fn make_default(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
	) -> Result<NodeRecord> {
		self.change(actor, origin, tenant, node, Change::Default)
			.await
	}
	pub(crate) async fn delete(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		input: NodeDeletionRequest,
	) -> Result<NodeRecord> {
		self.change(
			actor,
			origin,
			tenant,
			node,
			Change::Delete(input.confirmation),
		)
		.await
	}
	pub(crate) async fn restore(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
	) -> Result<NodeRecord> {
		self.change(actor, origin, tenant, node, Change::Restore)
			.await
	}
	pub(crate) async fn purge(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
	) -> Result<NodeRecord> {
		self.change(actor, origin, tenant, node, Change::Purge)
			.await
	}

	async fn change(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		name: String,
		change: Change,
	) -> Result<NodeRecord> {
		let name = domain::NodeName::parse(&name)?;
		let f = self.runtime.clone();
		let (node, deletion, bytes) = match actor {
			Actor::DeploymentAdministrator => {
				f.store
					.pool
					.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
						let actor = if let Some(origin) = &origin {
							origin.require_deployment_administrator(tx, true).await?;
							AdministrationActor::DeploymentAdministrator {
								identity: origin.identity_id,
							}
						} else {
							AdministrationActor::DeploymentAdministratorBearer
						};
						crate::apps::identity::models::AuthorizationBundle::lock_snapshot(
							tx, &tenant, true,
						)
						.await?;
						change_in(&f, tx, &tenant, name.as_str(), &change, &actor).await
					})
					.await?
			}
			actor => {
				if matches!(change, Change::Purge) {
					return Err(Error::Forbidden);
				}
				self.administrations()
					.run(actor, origin, &tenant, async |administrator| {
						f.store
							.pool
							.atomic(&administrator.scope()?, async |tx| {
								// The policy fence protects current authorization and Node admission.
								let snapshot = administrator.lock(tx, true).await?;
								let current = nodes::get(tx, &tenant, name.as_str()).await?;
								administrator
									.require(
										tx,
										&snapshot,
										change.permission(),
										name.as_str(),
										json!({"label": current.label,"default":current.default}),
									)
									.await?;
								change_in(
									&f,
									tx,
									&tenant,
									name.as_str(),
									&change,
									&AdministrationActor::TenantAdministrator(
										administrator.clone(),
									),
								)
								.await
							})
							.await
					})
					.await?
			}
		};
		let mut record = self.record(node, deletion);
		record.retained_storage_bytes = Some(bytes);
		Ok(record)
	}
}

enum Change {
	Default,
	Delete(String),
	Restore,
	Purge,
}
impl Change {
	fn permission(&self) -> &'static str {
		if matches!(self, Self::Default) {
			rules::NODE_UPDATE
		} else {
			rules::NODE_DELETE
		}
	}
	fn audit(&self) -> &'static str {
		match self {
			Self::Default => "node.default",
			Self::Delete(_) => rules::NODE_DELETE,
			Self::Restore => "node.restore",
			Self::Purge => "node.purge",
		}
	}
}

/// The same Tenant lock serializes creation, defaults, deletion, restoration,
/// Billing Account attachment and Tier changes.
async fn change_in(
	f: &Federation,
	tx: &mut reinhardt::db::orm::AtomicTransaction,
	tenant: &str,
	name: &str,
	change: &Change,
	actor: &AdministrationActor,
) -> Result<(Node, Option<NodeDeletion>, u64)> {
	nodes::lock_tenant(tx, tenant).await?;
	let mut node = nodes::get(tx, tenant, name).await?;
	let current = node_deletions::current_locked(tx, tenant, name).await?;
	let deletion = match change {
		Change::Default => {
			if current.is_some() {
				return Err(Error::Conflict("node_deleted".into()));
			}
			node = nodes::set_default(tx, tenant, name).await?;
			None
		}
		Change::Delete(confirmation) => {
			if confirmation != name {
				return Err(Error::Invalid(
					"confirmation must match the Node name".into(),
				));
			}
			if current.is_some() {
				return Err(Error::Conflict("node_deleted".into()));
			}
			domain::deletion::check_delete(node.default, nodes::count_live(tx, tenant).await?)?;
			let deletion = node_deletions::record(
				tx,
				tenant,
				name,
				actor,
				f.config.tenancy.node_deletion_grace_days,
			)
			.await?;
			crate::apps::execution::repositories::node_cancellations::record(f, tx, &deletion)
				.await?;
			Some(deletion)
		}
		Change::Restore => {
			let row = current.ok_or_else(|| Error::Conflict("the Node is not deleted".into()))?;
			let tier = f
				.config
				.tier_limits
				.nodes(&TenantName::new(tenant)?)
				.await?;
			domain::check_creation(
				domain::node_limit(&f.config.tenancy, tier),
				nodes::count_live(tx, tenant).await?,
			)
			.map_err(|domain::NodeLimitReached| Error::Conflict("node_limit_reached".into()))?;
			node_deletions::restore(tx, &row, actor).await?;
			None
		}
		Change::Purge => {
			let row = current.ok_or_else(|| Error::Conflict("the Node is not deleted".into()))?;
			Some(node_deletions::request_purge(tx, row).await?)
		}
	};
	actor
		.record_node(
			tx,
			AdministrationEvent {
				action: change.audit(),
				identity: None,
				registration: None,
				mapping: None,
				tenant: Some(tenant),
				subject: None,
				invitation: None,
			},
			name,
		)
		.await?;
	let bytes = super::node_purge::retained_storage_bytes_in(tx, tenant, name).await?;
	Ok((node, deletion, bytes))
}

/// Counts live Nodes within the caller's Tenant scope. Capacity decisions
/// hold `lock_capacity` until their local admission state commits.
pub async fn count_live(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<u32> {
	nodes::count_live(tx, tenant).await
}

/// Creates a Node under the Tenant's lock, within "Nodes per Tenant" narrowed
/// by the Tier, and records the creation.
async fn create_in(
	f: &Federation,
	tx: &mut reinhardt::db::orm::AtomicTransaction,
	tenant: &str,
	label: &str,
	creator: Creator,
	actor: &AdministrationActor,
) -> Result<Node> {
	nodes::lock_tenant(tx, tenant).await?;
	let count = nodes::count_live(tx, tenant).await?;
	let tier = f
		.config
		.tier_limits
		.nodes(&TenantName::new(tenant)?)
		.await?;
	domain::check_creation(domain::node_limit(&f.config.tenancy, tier), count)
		.map_err(|domain::NodeLimitReached| Error::NodeLimitReached)?;
	let node = nodes::create(tx, tenant, label, false, creator).await?;
	actor
		.record_node(
			tx,
			AdministrationEvent {
				action: rules::NODE_CREATE,
				identity: None,
				registration: None,
				mapping: None,
				tenant: Some(tenant),
				subject: None,
				invitation: None,
			},
			node.name.as_str(),
		)
		.await?;
	Ok(node)
}

/// Hold the same capacity lock for Node creation and changes to its Tier limit.
/// The caller retains the transaction until the new admission state is visible.
pub async fn lock_capacity(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<()> {
	nodes::lock_tenant(tx, tenant).await
}

async fn relabel_in(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	name: &str,
	label: &str,
	actor: &AdministrationActor,
) -> Result<Node> {
	let node = nodes::relabel(tx, tenant, name, label).await?;
	actor
		.record_node(
			tx,
			AdministrationEvent {
				action: rules::NODE_UPDATE,
				identity: None,
				registration: None,
				mapping: None,
				tenant: Some(tenant),
				subject: None,
				invitation: None,
			},
			node.name.as_str(),
		)
		.await?;
	Ok(node)
}
