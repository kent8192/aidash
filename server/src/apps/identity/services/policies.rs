use crate::apps::identity::models::{AdministrationActor, AuthorizationCatalog};
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::repositories::catalog_approvals::TenantCatalog;
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	Authorization, Snapshot,
	catalog::Binding,
	identity::{Credential, IssuedCredential},
	policy::{Decision, Evaluation},
};
use crate::authorization::{access::Access, identity::Actor, identity::SubjectIdentity};
use crate::{Error, Result, federation::Federation};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::Model;
use reinhardt::injectable;

use serde_json::Value;

use uuid::Uuid;

fn service(f: Federation) -> Authorization {
	Authorization { pool: f.store.pool }
}

fn control_service(f: Federation) -> Authorization {
	Authorization {
		pool: f.store.control_pool,
	}
}

use crate::apps::identity::serializers::policies::{
	AuthorizationPage, AuthorizationUpdate, CatalogInput, CredentialInput, CredentialPage,
	CredentialRevocation, PolicyReplacement,
};

/// A Tenant's policy, credentials and Catalog Approvals. A Deployment
/// Administrator manages any Tenant's; a Tenant's own subjects act through a
/// Mapping under its policy actions (#356, #357, #358).
/// Evaluation bodies are simulations of authority, never execution credentials.
#[derive(Clone)]
pub struct PolicyAdministration {
	runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PolicyAdministration {
	PolicyAdministration { runtime }
}

/// A Tenant route acts only for a subject of the Tenant it names.
fn tenant_subject(actor: Actor, tenant: &str) -> Result<SubjectIdentity> {
	match actor {
		Actor::Subject(identity) if identity.tenant == tenant => Ok(identity),
		_ => Err(Error::Forbidden),
	}
}

impl PolicyAdministration {
	fn administrations(&self) -> TenantAdministrations {
		TenantAdministrations {
			runtime: self.runtime.clone(),
		}
	}

	/// The Tenant's Catalog Approvals, under `registry.read`. A Deployment
	/// Administrator reads none (ADR 0029).
	pub(crate) async fn catalog(
		&self,
		actor: Actor,
		tenant: String,
		node: aidash_domain::nodes::NodeName,
	) -> Result<Vec<Binding>> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			let resource = access.resource(
				"registry",
				aidash_domain::identity::tenant_administration::COLLECTION,
				serde_json::json!({}),
			);
			access.require(&resource, "registry.read").await?;
			crate::apps::identity::repositories::nodes::get(
				&mut *access.tx,
				&tenant,
				node.as_str(),
			)
			.await?;
			Ok(AuthorizationCatalog::objects()
				.filter(AuthorizationCatalog::field_tenant().eq(identity.tenant.clone()))
				.filter(AuthorizationCatalog::field_node().eq(node.as_str().to_owned()))
				.order_by(&["entry_id", "entry_version"])
				.all_with_executor(&mut **access.tx)
				.await
				.map_err(FrameworkError::from)?
				.into_iter()
				.map(|row| Binding {
					tenant: row.tenant,
					node: node.clone(),
					entry_id: row.entry_key,
					entry_version: row.entry_version,
					enabled: row.enabled,
					revision: row.revision,
					restriction: row
						.restriction
						.and_then(|value| serde_json::from_value(value.0).ok()),
				})
				.collect())
		}
		.await;
		access.finish(result).await
	}
	pub(crate) async fn set_catalog(
		&self,
		actor: Actor,
		tenant: String,
		input: CatalogInput,
	) -> Result<Binding> {
		let approval = aidash_application::authorization::catalog::approvals::Approval {
			entry: input.entry,
			enabled: input.enabled,
			expected_revision: input.expected_revision,
			groups: input.groups,
			restriction: input.restriction,
		};
		Ok(self
			.approve(actor, tenant, input.node, vec![approval])
			.await?
			.remove(0))
	}
	/// Approvals of one dependency closure in one transaction.
	pub(crate) async fn approve(
		&self,
		actor: Actor,
		tenant: String,
		node: aidash_domain::nodes::NodeName,
		approvals: Vec<aidash_application::authorization::catalog::approvals::Approval>,
	) -> Result<Vec<Binding>> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
		if let Err(error) = crate::apps::identity::repositories::nodes::get(
			&mut **access.tx,
			&tenant,
			node.as_str(),
		)
		.await
		{
			return access.finish(Err(error)).await;
		}
		let result = aidash_application::authorization::catalog::approvals::approve(
			&mut TenantCatalog {
				access: &mut access,
				node: f.config.deployment.node(node),
			},
			&approvals,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
	pub(crate) async fn issue_credential(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: CredentialInput,
	) -> Result<IssuedCredential> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return service(f)
				.issue_credential(
					&tenant,
					&input.subject,
					input.expires_in_seconds,
					"deployment_administrator",
				)
				.await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.issue_credential(&f.store.pool, &input.subject, input.expires_in_seconds)
					.await
			})
			.await
	}
	pub(crate) async fn credentials(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: CredentialPage,
	) -> Result<Vec<Credential>> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return control_service(f)
				.credentials_page(&tenant, page.offset, page.active)
				.await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.credentials(&f.store.pool, page.offset, page.mine, page.active)
					.await
			})
			.await
	}
	pub(crate) async fn revoke_credential(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		(tenant, id): (String, Uuid),
	) -> Result<(reinhardt::StatusCode, CredentialRevocation)> {
		let f = self.runtime.clone();
		let credential = if matches!(actor, Actor::DeploymentAdministrator) {
			control_service(f.clone())
				.revoke_credential(&tenant, id)
				.await?
		} else {
			self.administrations()
				.run(actor, origin, &tenant, async |administrator| {
					// Revocation keeps its own pool, as a Deployment
					// Administrator's does, so a busy API pool cannot delay it.
					administrator
						.revoke_credential(&f.store.control_pool, id)
						.await
				})
				.await?
		};
		let pending_transactions =
			crate::transactions::authority::pending(&f, &tenant, Some(id)).await?;
		let status = if pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((
			status,
			CredentialRevocation {
				credential,
				pending_transactions,
			},
		))
	}
	pub(crate) async fn snapshot(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Snapshot> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return control_service(f).snapshot(&tenant).await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator.policy_snapshot(&f.store.pool).await
			})
			.await
	}
	pub(crate) async fn replace(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: AuthorizationUpdate,
	) -> Result<(reinhardt::StatusCode, PolicyReplacement)> {
		let f = self.runtime.clone();
		// The replacement itself refuses a deleted Tenant too, but its error
		// crosses the authorization port as a plain refusal.
		let deleted = f
			.store
			.pool
			.atomic(
				&aidash_application::tenancy::TenancyScope::tenant(&tenant)?,
				async |tx| {
					crate::apps::identity::repositories::tenant_deletions::is_deleted(tx, &tenant)
						.await
				},
			)
			.await?;
		if deleted {
			return Err(crate::Error::TenantDeleted);
		}
		let snapshot = if matches!(actor, Actor::DeploymentAdministrator) {
			// A browser session records which Deployment Administrator replaced it.
			let revision_actor = match &origin {
				Some(origin) => format!("deployment_administrator:oidc:{}", origin.identity_id),
				None => "deployment_administrator".to_owned(),
			};
			// A Tenant without a root record is refused with 404: only the Tenant
			// creation API makes Tenants (ADR 0050).
			control_service(f.clone())
				.replace(
					&tenant,
					input.expected_revision,
					input.bundle,
					&revision_actor,
				)
				.await?
		} else {
			self.administrations()
				.run(actor, origin, &tenant, async |administrator| {
					administrator
						.replace_policy(
							&f.store.control_pool,
							&AdministrationActor::TenantAdministrator(administrator.clone()),
							input.expected_revision,
							&input.bundle,
						)
						.await
				})
				.await?
		};
		let pending_transactions =
			crate::transactions::authority::pending(&f, &tenant, None).await?;
		let status = if pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((
			status,
			PolicyReplacement {
				snapshot,
				pending_transactions,
			},
		))
	}
	pub async fn transaction_revocations(&self, tenant: String) -> Result<Vec<Uuid>> {
		crate::transactions::authority::pending(&self.runtime, &tenant, None).await
	}
	pub async fn evaluate(&self, tenant: String, input: Evaluation) -> Result<Decision> {
		let f = self.runtime.clone();
		control_service(f).evaluate(&tenant, &input).await
	}
	pub(crate) async fn simulate(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: Evaluation,
	) -> Result<Decision> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return control_service(f).simulate(&tenant, &input).await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator.simulate(&f.store.pool, &input).await
			})
			.await
	}
	pub(crate) async fn revisions(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return service(f).revisions(&tenant, page.after, page.limit).await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.policy_revisions(&f.store.pool, page.after, page.limit)
					.await
			})
			.await
	}
	pub(crate) async fn decisions(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return service(f).decisions(&tenant, page.after, page.limit).await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.decisions(&f.store.pool, page.after, page.limit)
					.await
			})
			.await
	}
}
