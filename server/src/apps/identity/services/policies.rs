use crate::apps::identity::models::AuthorizationCatalog;
use crate::apps::identity::repositories::catalog_approvals::TenantCatalog;
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
	AuthorizationPage, AuthorizationUpdate, CatalogInput, CredentialInput, CredentialRevocation,
	PolicyReplacement,
};

/// Mounted inside the management router, behind the existing Deployment Administrator token.
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
	/// The Tenant's Catalog Approvals, under `registry.read`. A Deployment
	/// Administrator reads none (ADR 0029).
	pub(crate) async fn catalog(&self, actor: Actor, tenant: String) -> Result<Vec<Binding>> {
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
			Ok(AuthorizationCatalog::objects()
				.filter(AuthorizationCatalog::field_tenant().eq(identity.tenant.clone()))
				.order_by(&["entry_id", "entry_version"])
				.all_with_executor(&mut **access.tx)
				.await
				.map_err(FrameworkError::from)?
				.into_iter()
				.map(|row| Binding {
					tenant: row.tenant,
					entry_id: row.entry_key,
					entry_version: row.entry_version,
					enabled: row.enabled,
					revision: row.revision,
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
		};
		Ok(self.approve(actor, tenant, vec![approval]).await?.remove(0))
	}
	/// Approvals of one dependency closure in one transaction.
	pub(crate) async fn approve(
		&self,
		actor: Actor,
		tenant: String,
		approvals: Vec<aidash_application::authorization::catalog::approvals::Approval>,
	) -> Result<Vec<Binding>> {
		let f = self.runtime.clone();
		let identity = tenant_subject(actor, &tenant)?;
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
		let result = aidash_application::authorization::catalog::approvals::approve(
			&mut TenantCatalog {
				access: &mut access,
				node: &f.config.node_id,
			},
			&approvals,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
	pub(crate) async fn issue_credential(
		&self,
		tenant: String,
		input: CredentialInput,
	) -> Result<IssuedCredential> {
		let f = self.runtime.clone();
		service(f)
			.issue_credential(
				&tenant,
				&input.subject,
				input.expires_in_seconds,
				"deployment_administrator",
			)
			.await
	}
	pub async fn credentials(
		&self,
		tenant: String,
		page: crate::apps::workspaces::serializers::tasks::PageQuery,
	) -> Result<Vec<Credential>> {
		let f = self.runtime.clone();
		control_service(f)
			.credentials_page(&tenant, page.offset)
			.await
	}
	pub(crate) async fn revoke_credential(
		&self,
		(tenant, id): (String, Uuid),
	) -> Result<(reinhardt::StatusCode, CredentialRevocation)> {
		let f = self.runtime.clone();
		let credential = control_service(f.clone())
			.revoke_credential(&tenant, id)
			.await?;
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
	pub async fn snapshot(&self, tenant: String) -> Result<Snapshot> {
		let f = self.runtime.clone();
		control_service(f).snapshot(&tenant).await
	}
	pub(crate) async fn replace(
		&self,
		tenant: String,
		input: AuthorizationUpdate,
	) -> Result<(reinhardt::StatusCode, PolicyReplacement)> {
		let f = self.runtime.clone();
		// A Tenant without a root record is refused with 404: only the Tenant
		// creation API makes Tenants (ADR 0050).
		let snapshot = control_service(f.clone())
			.replace(
				&tenant,
				input.expected_revision,
				input.bundle,
				"deployment_administrator",
			)
			.await?;
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
	pub async fn simulate(&self, tenant: String, input: Evaluation) -> Result<Decision> {
		let f = self.runtime.clone();
		control_service(f).simulate(&tenant, &input).await
	}
	pub(crate) async fn revisions(
		&self,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		service(f).revisions(&tenant, page.after, page.limit).await
	}
	pub(crate) async fn decisions(
		&self,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		service(f).decisions(&tenant, page.after, page.limit).await
	}
}
