//! Node Registrations: which Nodes of other Tenants a Tenant's Node accepts
//! (#241). Tenant Administrators decide with `node_registration.update`, and a
//! Deployment Administrator may too. Registering a Node of another Deployment
//! pins the key that Deployment publishes.
use crate::apps::identity::models::{AdministrationActor, AdministrationEvent};
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::repositories::node_registrations as records;
use crate::apps::identity::serializers::node_registrations::{
	RegistrationInput, RegistrationRecord, RegistrationRevision,
};
use crate::apps::identity::serializers::peer::{HistoryPage, MappingPage};
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::authorization::identity::Actor;
use crate::federation::Federation;
use crate::{Error, Result};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::tenant_administration as rules;
use aidash_domain::nodes::{
	NodeAddress, NodeName,
	registration::{Counterpart, NodeRegistration, Requested},
};
use aidash_integrations::federation::directory::HttpDirectory;
use serde_json::json;

pub struct NodeRegistrations {
	runtime: Federation,
}

#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> NodeRegistrations {
	NodeRegistrations { runtime }
}

/// The resource a Node Registration is decided as: the registered Node's address.
fn attributes(node: &str, source: &NodeAddress) -> serde_json::Value {
	json!({"node": node, "source_node": source.to_string(), "source_deployment": source.deployment.as_str()})
}

impl NodeRegistrations {
	fn administrations(&self) -> TenantAdministrations {
		TenantAdministrations {
			runtime: self.runtime.clone(),
		}
	}

	pub(crate) async fn list(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		page: MappingPage,
	) -> Result<Vec<RegistrationRecord>> {
		crate::http::validate(&page)?;
		let node = NodeName::parse(&node)?;
		let f = self.runtime.clone();
		let rows = match actor {
			Actor::DeploymentAdministrator => {
				f.store
					.pool
					.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
						require_node(tx, &tenant, &node).await?;
						records::list(
							tx,
							&tenant,
							node.as_str(),
							page.offset as u64,
							page.limit as u64,
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
									.require(
										tx,
										&snapshot,
										rules::NODE_REGISTRATION_READ,
										node.as_str(),
										json!({"node": node.as_str()}),
									)
									.await?;
								require_node(tx, &tenant, &node).await?;
								records::list(
									tx,
									&tenant,
									node.as_str(),
									page.offset as u64,
									page.limit as u64,
								)
								.await
							})
							.await
					})
					.await?
			}
		};
		Ok(rows.into_iter().map(Into::into).collect())
	}

	pub(crate) async fn history(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		page: HistoryPage,
	) -> Result<Vec<RegistrationRevision>> {
		crate::http::validate(&page)?;
		let node = NodeName::parse(&node)?;
		let f = self.runtime.clone();
		let rows = match actor {
			Actor::DeploymentAdministrator => {
				f.store
					.pool
					.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
						require_node(tx, &tenant, &node).await?;
						records::history(tx, &tenant, node.as_str(), page.after, page.limit as u64)
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
									.require(
										tx,
										&snapshot,
										rules::NODE_REGISTRATION_READ,
										node.as_str(),
										json!({"node": node.as_str()}),
									)
									.await?;
								require_node(tx, &tenant, &node).await?;
								records::history(
									tx,
									&tenant,
									node.as_str(),
									page.after,
									page.limit as u64,
								)
								.await
							})
							.await
					})
					.await?
			}
		};
		Ok(rows
			.into_iter()
			.map(|(sequence, registration)| RegistrationRevision {
				sequence,
				registration: registration.into(),
			})
			.collect())
	}

	/// Registers, or replaces, one Node at `node`. The other Deployment is asked
	/// for its identity outside any transaction, after the caller's authority
	/// is known and before it is checked again with the write.
	pub(crate) async fn set(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		input: RegistrationInput,
	) -> Result<RegistrationRecord> {
		let node = NodeName::parse(&node)?;
		let requested = input.requested(&self.runtime.config.deployment, &node)?;
		let f = self.runtime.clone();
		let stored = match actor {
			Actor::DeploymentAdministrator => {
				f.store
					.pool
					.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
						require_node(tx, &tenant, &node).await
					})
					.await?;
				let (counterpart, preserve_keys) =
					self.counterpart(&tenant, &node, &input, &requested).await?;
				f.store
					.pool
					.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
						require_node(tx, &tenant, &node).await?;
						write(
							tx,
							&tenant,
							&node,
							&input,
							&counterpart,
							preserve_keys,
							&AdministrationActor::DeploymentAdministratorBearer,
							"deployment_administrator",
						)
						.await
					})
					.await?
			}
			actor => {
				self.administrations()
					.run(actor, origin, &tenant, async |administrator| {
						let decide = async |tx: &mut reinhardt::db::orm::AtomicTransaction| {
							let snapshot = administrator.lock(tx, false).await?;
							administrator
								.require(
									tx,
									&snapshot,
									rules::NODE_REGISTRATION_UPDATE,
									&input.source_node.to_string(),
									attributes(node.as_str(), &input.source_node),
								)
								.await
						};
						f.store
							.pool
							.atomic(&administrator.scope()?, async |tx| {
								decide(tx).await?;
								require_node(tx, &tenant, &node).await
							})
							.await?;
						let (counterpart, preserve_keys) =
							self.counterpart(&tenant, &node, &input, &requested).await?;
						f.store
							.pool
							.atomic(&administrator.scope()?, async |tx| {
								decide(tx).await?;
								write(
									tx,
									&tenant,
									&node,
									&input,
									&counterpart,
									preserve_keys,
									&AdministrationActor::TenantAdministrator(
										administrator.clone(),
									),
									&format!("oidc:{}", administrator.identity_id),
								)
								.await
							})
							.await
					})
					.await?
			}
		};
		Ok(stored.into())
	}

	/// What the registration pins: nothing for a Node of this Deployment, the
	/// key the other Deployment publishes otherwise, or what the registration
	/// already pins when the request only disables it.
	async fn counterpart(
		&self,
		tenant: &str,
		node: &NodeName,
		input: &RegistrationInput,
		requested: &Requested,
	) -> Result<(Counterpart, bool)> {
		let f = &self.runtime;
		match requested {
			Requested::Keep => f
				.store
				.pool
				.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
					require_node(tx, tenant, node).await?;
					records::get(tx, tenant, node.as_str(), &input.source_node).await
				})
				.await?
				.map(|current| (current.counterpart, true))
				.ok_or_else(|| Error::NotFound("node registration not found".into())),
			Requested::SameDeployment => {
				let holder = f
					.store
					.pool
					.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
						require_node(tx, tenant, node).await?;
						records::name_tenant(tx, input.source_node.node.as_str()).await
					})
					.await?;
				match holder.as_deref() {
					None => Err(Error::Unprocessable(
						"no Node of this Deployment has that name".into(),
					)),
					Some(holder) if holder == tenant => Err(Error::Unprocessable(
						"that Node belongs to this Tenant; its policy decides within the Tenant"
							.into(),
					)),
					Some(_) => Ok((Counterpart::SameDeployment, false)),
				}
			}
			Requested::OtherDeployment { endpoint, key_id } => {
				// Changing what the registration allows leaves its pin alone, and
				// must not depend on the other Deployment answering.
				let current = f
					.store
					.pool
					.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
						require_node(tx, tenant, node).await?;
						records::get(tx, tenant, node.as_str(), &input.source_node).await
					})
					.await?;
				if let Some(Counterpart::OtherDeployment {
					endpoint: pinned,
					key,
				}) = current.map(|current| current.counterpart)
				{
					if key.key_id != *key_id {
						return Err(Error::Conflict(
							"use independent key confirmation to replace an existing pin".into(),
						));
					}
					if pinned == *endpoint {
						// This is an authorization-only edit. The repeated pin is
						// not a crypto precondition; preserve any manual or normal
						// key change that wins before the authorization UPDATE.
						return Ok((
							Counterpart::OtherDeployment {
								endpoint: pinned,
								key,
							},
							true,
						));
					}
				}
				let key = aidash_application::federation::registrations::pin(
					&HttpDirectory::new(f.store.keyless.clone())?,
					&input.source_node.deployment,
					endpoint,
					key_id,
				)
				.await?;
				Ok((
					Counterpart::OtherDeployment {
						endpoint: endpoint.clone(),
						key,
					},
					false,
				))
			}
		}
	}
}

async fn require_node(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	node: &NodeName,
) -> Result<()> {
	super::remote::deployment_administrator::tenant(tx, tenant).await?;
	crate::apps::identity::repositories::nodes::get(tx, tenant, node.as_str())
		.await
		.map(|_| ())
}

/// Writes the registration and its revision, and records the change.
async fn write(
	tx: &mut reinhardt::db::orm::AtomicTransaction,
	tenant: &str,
	node: &NodeName,
	input: &RegistrationInput,
	counterpart: &Counterpart,
	preserve_keys: bool,
	by: &AdministrationActor,
	stamp: &str,
) -> Result<NodeRegistration> {
	let row = records::Write {
		tenant,
		node: node.as_str(),
		source_node: &input.source_node,
		counterpart,
		atomic_transactions: input.atomic_transactions,
		enabled: input.enabled,
		actor: stamp,
		preserve_keys,
	};
	let stored = if input.expected_revision == 0 {
		records::insert(tx, &row).await?
	} else {
		records::update(tx, &row, input.expected_revision).await?
	}
	.ok_or_else(|| Error::Conflict("node registration revision changed".into()))?;
	records::append_history(tx, &stored).await?;
	by.record(
		tx,
		AdministrationEvent {
			action: rules::NODE_REGISTRATION_UPDATE,
			identity: None,
			registration: None,
			mapping: None,
			tenant: Some(tenant),
			subject: None,
			invitation: None,
		},
	)
	.await?;
	Ok(stored)
}
