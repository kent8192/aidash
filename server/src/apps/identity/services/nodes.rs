//! A Tenant's Nodes: listing, creating within "Nodes per Tenant" and
//! relabeling (#241). Members list them; Tenant Administrators create,
//! relabel, delete and restore them. A Deployment Administrator only lists
//! deleted Nodes and purges one immediately (ADR 0083, ADR 0102).
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
			// Only immediate purge is retained for a Deployment Administrator
			// (ADR 0083, ADR 0102); it lists deleted Nodes to choose one.
			Actor::DeploymentAdministrator if page.include_deleted => {
				let scope = TenancyScope::tenant(&tenant)?;
				f.store
					.pool
					.atomic(&scope, async |tx| {
						super::remote::deployment_administrator::tenant(tx, &tenant).await?;
						self.records_in(tx, &tenant, &page).await
					})
					.await?
			}
			Actor::DeploymentAdministrator => return Err(Error::Forbidden),
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
		let node = self
			.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				let exclusive = administrator
					.may_change(&f.store.pool, rules::NODE_CREATE)
					.await?;
				f.store
					.pool
					.atomic(&administrator.scope()?, async |tx| {
						let mut snapshot = administrator
							.lock_change(tx, exclusive, rules::NODE_CREATE)
							.await?;
						administrator
							.require(tx, &snapshot, rules::NODE_CREATE, &tenant, json!({}))
							.await?;
						let actor = AdministrationActor::TenantAdministrator(administrator.clone());
						create_in(
							&f,
							tx,
							&tenant,
							&label,
							Creator::Identity(administrator.identity_id).into(),
							&actor,
							&mut snapshot,
						)
						.await
					})
					.await
			})
			.await?;
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
		let node = self
			.administrations()
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
							&AdministrationActor::TenantAdministrator(administrator.clone()),
						)
						.await
					})
					.await
			})
			.await?;
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
			// A Deployment Administrator only bypasses the grace period (ADR
			// 0083); the rest of the lifecycle is Tenant administration (ADR 0102).
			Actor::DeploymentAdministrator if !matches!(change, Change::Purge) => {
				return Err(Error::Forbidden);
			}
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
						let exclusive = administrator
							.may_change(&f.store.pool, change.permission())
							.await?;
						f.store
							.pool
							.atomic(&administrator.scope()?, async |tx| {
								// The policy fence protects current authorization and Node admission.
								let snapshot = administrator
									.lock_change(tx, exclusive, change.permission())
									.await?;
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
			crate::apps::execution::services::node_cancellations::record_deletion_in(
				f,
				tx,
				deletion.id,
				tenant,
				name,
			)
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
			// The caller holds the policy exclusively, as Node creation does.
			let restored = domain::NodeName::parse(name)?;
			crate::apps::identity::repositories::catalog_node_copy::reconcile_restored(
				tx,
				tenant,
				&f.config.deployment,
				&restored,
			)
			.await?;
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
				subject: concerned_subject(actor),
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
	creator: nodes::NodeCreator,
	actor: &AdministrationActor,
	snapshot: &mut crate::authorization::Snapshot,
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
	crate::apps::identity::repositories::catalog_node_copy::copy(
		tx,
		tenant,
		&f.config.deployment,
		&node.name,
		snapshot,
	)
	.await?;

	actor
		.record_node(
			tx,
			AdministrationEvent {
				action: rules::NODE_CREATE,
				identity: None,
				registration: None,
				mapping: None,
				tenant: Some(tenant),
				subject: concerned_subject(actor),
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

/// The person an Agent's chain acted as, recorded as the history event's
/// concerned subject because the history has no actor-subject column.
fn concerned_subject(actor: &AdministrationActor) -> Option<&str> {
	match actor {
		AdministrationActor::Agent { subject } => Some(subject),
		_ => None,
	}
}

/// The person an Agent's Run acts as, with the credential it was admitted
/// with (#1012, ADR 0143). A Run that arrived through Federation, and a Run
/// whose chain starts at no person, has nobody to act as.
struct AgentCaller {
	identity: crate::authorization::identity::SubjectIdentity,
}

async fn agent_caller(f: &Federation, run: &aidash_domain::Run) -> Result<AgentCaller> {
	use aidash_application::ports::authorization::ExecutionGrantRepository;
	if !run.is_local(f.config.deployment.as_str()) {
		return Err(Error::Invalid(
			"a Run that arrived through Federation cannot create or delete Nodes here".into(),
		));
	}
	let grant = crate::apps::identity::repositories::execution::Grants { store: &f.store }
		.grant(run.id)
		.await?
		.ok_or(Error::Forbidden)?;
	let owner = crate::apps::identity::repositories::oauth_connections::owner_of_run(&f.store, run)
		.await?
		.ok_or_else(|| {
			Error::Invalid(
				"Nodes are created only for a person, and this conversation starts at no person"
					.into(),
			)
		})?;
	Ok(AgentCaller {
		identity: crate::authorization::identity::SubjectIdentity {
			http_session: None,
			credential_id: grant.credential_id,
			tenant: owner.tenant,
			subject: owner.subject,
		},
	})
}

type Verdict = (
	aidash_domain::policy::Evaluation,
	aidash_domain::policy::Decision,
);

/// One decision of the Tenant's policy for the root person, recorded in the
/// decision log as a Tenant Administrator's is. A denial is kept to be
/// recorded after its transaction rolls back.
fn decide(
	f: &Federation,
	identity: &crate::authorization::identity::SubjectIdentity,
	snapshot: &crate::authorization::Snapshot,
	action: &str,
	id: &str,
	attributes: serde_json::Value,
) -> Verdict {
	let evaluation = aidash_domain::policy::Evaluation {
		subject: identity.subject.clone(),
		action: action.into(),
		resource: aidash_domain::policy::Resource {
			tenant: identity.tenant.clone(),
			kind: rules::resource_kind(action).into(),
			id: id.into(),
			attributes,
		},
		environment: json!({"node_id": f.config.deployment.as_str(), "transport": "agent"}),
	};
	let mut decision = snapshot.bundle.evaluate(&evaluation);
	decision.revision = snapshot.revision;
	(evaluation, decision)
}

/// Commits a denial whose transaction rolled back, so the decision log shows it.
async fn record_denial(f: &Federation, tenant: &str, denial: Option<Verdict>) -> Result<()> {
	let Some((evaluation, decision)) = denial else {
		return Ok(());
	};
	f.store
		.pool
		.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
			crate::apps::identity::models::AuthorizationDecision::append(
				tx,
				tenant,
				&evaluation,
				&decision,
			)
			.await
		})
		.await
}

/// A Node tool call by an Agent (#1012, ADR 0143): `node_create` or
/// `node_delete`, acting as the person at the root of the Run's chain. Both are
/// Authority Changes: the harness releases the Run's own lease first.
pub async fn agent_tool(
	f: &Federation,
	run: &aidash_domain::Run,
	key: &str,
	operation: &str,
	input: serde_json::Value,
) -> Result<serde_json::Value> {
	use aidash_application::nodes::tool;
	let caller = agent_caller(f, run).await?;
	// Persist a removed Tenant Binding's disablement before any lease.
	caller.identity.check_binding(&f.store.pool).await?;
	match operation {
		"node_create" => {
			let input: tool::Create =
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
			let label = domain::label(&input.label)?;
			let node = agent_create(f, run, &caller, key, &label).await?;
			Ok(json!({
				"node": node.name.as_str(),
				"address": f.config.deployment.node(node.name.clone()).to_string(),
				"label": node.label,
				"created_at": node.created_at,
			}))
		}
		"node_delete" => {
			let input: tool::Delete =
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
			let name = domain::NodeName::parse(&input.node)?;
			if name == run.node {
				return Err(Error::Invalid(
					"a Run cannot delete the Node it runs on".into(),
				));
			}
			let (node, deletion) = agent_delete(f, &caller, name.as_str()).await?;
			Ok(json!({
				"node": node.name.as_str(),
				"label": node.label,
				"deleted": true,
				"restorable_until": deletion.as_ref().map(|row| row.purge_after),
			}))
		}
		_ => Err(Error::Invalid("unknown Node tool".into())),
	}
}

async fn agent_create(
	f: &Federation,
	run: &aidash_domain::Run,
	caller: &AgentCaller,
	key: &str,
	label: &str,
) -> Result<Node> {
	let identity = &caller.identity;
	let tenant = identity.tenant.clone();
	let key = aidash_domain::nodes::agents::creation_key(key)?;
	// Decided on a policy read before the lock, as a Tenant Administrator's
	// change is: a person no policy could allow `node.create` takes the
	// ordinary lock, so the Tenant's work never waits for a refusal (ADR 0121).
	let read = crate::apps::identity::services::core::Authorization {
		pool: f.store.pool.clone(),
	}
	.snapshot(&tenant)
	.await?;
	let exclusive = read.bundle.may_allow(&identity.subject, rules::NODE_CREATE);
	let policy = f.config.dashboard_policy();
	let denial = std::sync::Mutex::new(None);
	let result = f
		.store
		.pool
		.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
			let mut snapshot = identity.lock_native(tx, exclusive, policy.as_ref()).await?;
			if !exclusive
				&& snapshot
					.bundle
					.may_allow(&identity.subject, rules::NODE_CREATE)
			{
				return Err(Error::Conflict("authorization revision changed".into()));
			}
			// A retried call returns the Node its first call created.
			if let Some(existing) = nodes::created_by_call(tx, &tenant, run.id, key).await? {
				return Ok(existing);
			}
			let (evaluation, decision) = decide(
				f,
				identity,
				&snapshot,
				rules::NODE_CREATE,
				&tenant,
				json!({}),
			);
			if !decision.allowed {
				*denial.lock().expect("denial lock") = Some((evaluation, decision));
				return Err(Error::Forbidden);
			}
			crate::apps::identity::models::AuthorizationDecision::append(
				tx,
				&tenant,
				&evaluation,
				&decision,
			)
			.await?;
			create_in(
				f,
				tx,
				&tenant,
				label,
				nodes::NodeCreator::Agent(nodes::AgentCreation {
					subject: identity.subject.clone(),
					run: run.id,
					task: run.task_id,
					key: key.to_owned(),
				}),
				&AdministrationActor::Agent {
					subject: identity.subject.clone(),
				},
				&mut snapshot,
			)
			.await
		})
		.await;
	match result {
		Err(Error::Forbidden) => {
			record_denial(f, &tenant, denial.into_inner().expect("denial lock")).await?;
			Err(Error::Invalid(
				"the person who started this conversation may not create Nodes in this Tenant (node.create)"
					.into(),
			))
		}
		Err(Error::NodeLimitReached) => Err(Error::Invalid(
			"the Tenant already holds as many Nodes as its plan allows".into(),
		)),
		result => result,
	}
}

async fn agent_delete(
	f: &Federation,
	caller: &AgentCaller,
	name: &str,
) -> Result<(Node, Option<NodeDeletion>)> {
	type Creators = std::collections::BTreeMap<String, aidash_domain::nodes::agents::NodeCreator>;
	let identity = &caller.identity;
	let tenant = identity.tenant.clone();
	let scope = TenancyScope::tenant(&tenant)?;
	let names = [name.to_owned()];
	let own = |creators: &Creators| {
		creators
			.get(name)
			.and_then(|creator| creator.root_subject())
			== Some(identity.subject.as_str())
	};
	// Decided before the lock: a Node a chain of this person created, or a
	// person some policy could allow `node.delete`, takes the policy
	// exclusively as a Tenant Administrator's deletion does (ADR 0121).
	let read = crate::apps::identity::services::core::Authorization {
		pool: f.store.pool.clone(),
	}
	.snapshot(&tenant)
	.await?;
	let creator_read = f
		.store
		.pool
		.atomic(&scope, async |tx| {
			nodes::creators(tx, &tenant, Some(names.as_slice())).await
		})
		.await?;
	let exclusive =
		own(&creator_read) || read.bundle.may_allow(&identity.subject, rules::NODE_DELETE);
	let policy = f.config.dashboard_policy();
	let denial = std::sync::Mutex::new(None);
	let result = f
		.store
		.pool
		.atomic(&scope, async |tx| {
			let snapshot = identity.lock_native(tx, exclusive, policy.as_ref()).await?;
			if !exclusive
				&& snapshot
					.bundle
					.may_allow(&identity.subject, rules::NODE_DELETE)
			{
				return Err(Error::Conflict("authorization revision changed".into()));
			}
			let current = nodes::get(tx, &tenant, name).await?;
			let creators = nodes::creators(tx, &tenant, Some(names.as_slice())).await?;
			let creator = creators
				.get(name)
				.ok_or_else(|| Error::NotFound("node not found".into()))?;
			let holds = if own(&creators) {
				false
			} else {
				let (evaluation, decision) = decide(
					f,
					identity,
					&snapshot,
					rules::NODE_DELETE,
					name,
					json!({"label": current.label, "default": current.default}),
				);
				let allowed = decision.allowed;
				if allowed {
					crate::apps::identity::models::AuthorizationDecision::append(
						tx,
						&tenant,
						&evaluation,
						&decision,
					)
					.await?;
				} else {
					*denial.lock().expect("denial lock") = Some((evaluation, decision));
				}
				allowed
			};
			aidash_domain::nodes::agents::may_delete(creator, &identity.subject, holds)
				.map_err(|_| Error::Forbidden)?;
			let (node, deletion, _) = change_in(
				f,
				tx,
				&tenant,
				name,
				&Change::Delete(name.to_owned()),
				&AdministrationActor::Agent {
					subject: identity.subject.clone(),
				},
			)
			.await?;
			Ok((node, deletion))
		})
		.await;
	match result {
		Err(Error::Forbidden) => {
			record_denial(f, &tenant, denial.into_inner().expect("denial lock")).await?;
			Err(Error::Invalid(
				"only a Node that a conversation of the person who started this one created can be deleted, unless they may delete Nodes (node.delete)"
					.into(),
			))
		}
		result => result,
	}
}
