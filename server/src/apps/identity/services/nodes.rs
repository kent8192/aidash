//! A Tenant's Nodes: listing, creating within "Nodes per Tenant" and
//! relabeling (#241). Members list them; Tenant Administrators create and
//! relabel them, as do Deployment Administrators.
use crate::apps::identity::models::{AdministrationActor, AdministrationEvent, Tenant};
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::repositories::{nodes, tenants::Creator};
use crate::apps::identity::serializers::nodes::{NodeCreation, NodeLabelUpdate, NodeRecord};
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

	fn record(&self, node: Node) -> NodeRecord {
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
		}
	}

	pub(crate) async fn list(&self, actor: Actor, tenant: String) -> Result<Vec<NodeRecord>> {
		let f = &self.runtime;
		let rows = match actor {
			Actor::DeploymentAdministrator => {
				let scope = TenancyScope::tenant(&tenant)?;
				f.store
					.pool
					.atomic(&scope, async |tx| {
						if !Tenant::exists(tx, &tenant).await? {
							return Err(Error::NotFound("tenant not found".into()));
						}
						nodes::list(tx, &tenant).await
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
					access.require(&resource, rules::NODE_READ).await?;
					let (tx, tenant) = access.tenant_transaction();
					nodes::list(tx, tenant).await
				}
				.await;
				access.finish(result).await?
			}
		};
		Ok(rows.into_iter().map(|node| self.record(node)).collect())
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
						if !Tenant::exists(tx, &tenant).await? {
							return Err(Error::NotFound("tenant not found".into()));
						}
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
		Ok(self.record(node))
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
						nodes::relabel(tx, &tenant, name.as_str(), &label).await
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
								let current = nodes::get(tx, &tenant, name.as_str()).await?;
								administrator
									.require(
										tx,
										&snapshot,
										rules::NODE_UPDATE,
										name.as_str(),
										json!({"label": current.label, "default": current.default}),
									)
									.await?;
								nodes::relabel(tx, &tenant, name.as_str(), &label).await
							})
							.await
					})
					.await?
			}
		};
		Ok(self.record(node))
	}
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
	let count = nodes::count(tx, tenant).await?;
	let tier = f
		.config
		.tier_limits
		.nodes(&TenantName::new(tenant)?)
		.await?;
	domain::check_creation(domain::node_limit(&f.config.tenancy, tier), count)
		.map_err(|domain::NodeLimitReached| Error::NodeLimitReached)?;
	let node = nodes::create(tx, tenant, label, false, creator).await?;
	actor
		.record(
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
		)
		.await?;
	Ok(node)
}
