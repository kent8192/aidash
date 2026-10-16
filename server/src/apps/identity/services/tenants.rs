//! The Tenant creation API and Tenant labels. One API creates every Tenant:
//! a signed-in External Identity where the Tenancy Rules allow it, and a
//! Deployment Administrator always. Subject Credentials, agents and peers never.
use crate::apps::identity::models::{
	AdministrationActor, AdministrationEvent, DashboardDeploymentAdministratorGrant,
	DashboardIdentity, Tenant, database_time,
};
use crate::apps::identity::oidc::{
	BrowserOrigin, csrf_allowed, required_config, session_from_headers,
};
use crate::apps::identity::repositories::tenants::{self, Created, Creator};
use crate::apps::identity::serializers::tenants::{
	CreatedTenant, LabelUpdate, TenantCreation, TenantLabel, TenantRecord,
};
use crate::apps::identity::services::http_auth::bearer;
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::authorization::{access::NativeAccess, identity::Actor};
use crate::federation::Federation;
use crate::http::validate;
use crate::{Error, Result};
use aidash_domain::identity::tenancy::{self, TENANT_UPDATE};
use aidash_domain::policy::Resource;
use http::{HeaderMap, Method};
use reinhardt::db::orm::Model as _;
use reinhardt::injectable;
use serde_json::json;

#[derive(Clone)]
pub struct Tenants {
	pub runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> Tenants {
	Tenants { runtime }
}

impl From<Created> for CreatedTenant {
	fn from(created: Created) -> Self {
		Self {
			name: created.name,
			label: created.label,
			created_at: created.created_at,
			mapping_id: created.mapping,
		}
	}
}

impl From<Tenant> for TenantRecord {
	fn from(tenant: Tenant) -> Self {
		Self {
			name: tenant.name,
			label: tenant.label,
			personal: tenant.personal,
			created_at: tenant.created_at,
			created_by_kind: tenant.created_by_kind,
		}
	}
}

impl Tenants {
	/// The Deployment Administrator bearer token, a browser session in the
	/// `deployment_administrator` context, or any other browser session as the External
	/// Identity itself. A session needs no Mapping. Every other bearer token,
	/// such as a Subject Credential, is refused.
	async fn creator(&self, headers: &HeaderMap) -> Result<Creator> {
		let f = &self.runtime;
		let desktop = crate::apps::identity::repositories::desktop::access_token(headers).is_some();
		if !desktop && let Some(token) = bearer(headers) {
			return if crate::config::same_secret(token, &f.config.api_token) {
				Ok(Creator::DeploymentAdministrator(None))
			} else {
				Err(Error::Forbidden)
			};
		}
		let config = required_config(f)?;
		let session = session_from_headers(f, headers).await?;
		if !csrf_allowed(config, headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let identity = session.identity_id();
		let context = headers
			.get("x-aidash-context")
			.and_then(|value| value.to_str().ok());
		if context == Some("deployment_administrator") {
			let lease = f.store.orm_connection()?;
			if !DashboardDeploymentAdministratorGrant::enabled_for_identity(
				&mut lease.handle(),
				identity,
			)
			.await?
			{
				return Err(Error::Forbidden);
			}
			return Ok(Creator::DeploymentAdministrator(Some(identity)));
		}
		Ok(Creator::Identity(identity))
	}

	pub(crate) async fn create(
		&self,
		headers: HeaderMap,
		input: TenantCreation,
	) -> Result<CreatedTenant> {
		validate(&input)?;
		let label = tenancy::label(&input.label)?;
		let creator = self.creator(&headers).await?;
		let f = self.runtime.clone();
		let lease = f.store.orm_connection()?;
		let created = lease
			.handle()
			.atomic(async |tx| {
				if let Creator::Identity(identity) = creator {
					tenants::lock_creator(tx, identity).await?;
					// Locked as Personal Tenant provisioning locks it: a
					// disablement committing meanwhile waits, or is seen here.
					let account = DashboardIdentity::objects()
						.filter(DashboardIdentity::field_id().eq(identity))
						.select_for_update()
						.all_with_executor(tx)
						.await
						.map_err(reinhardt::core::exception::Error::from)?
						.pop()
						.ok_or(Error::Unauthorized)?;
					let pool =
						(!account.gcip_tenant.is_empty()).then_some(account.gcip_tenant.as_str());
					if account.disabled_at.is_some()
						|| !f
							.config
							.dashboard_policy()
							.is_some_and(|policy| policy.unbound(pool))
					{
						return Err(Error::Forbidden);
					}
					let now = database_time(tx).await?;
					let (recent, total) = tenants::creations(tx, identity, now).await?;
					tenancy::check_creation(&f.config.tenancy, now, &recent, total)
						.map_err(Error::TenantCreation)?;
				}
				tenants::create(tx, &label, false, creator).await
			})
			.await?;
		Ok(created.into())
	}

	/// One page of Tenants, newest first, only for Deployment Administrators:
	/// a subject of one Tenant never enumerates the others.
	pub(crate) async fn list(&self, actor: Actor, page: PageQuery) -> Result<Vec<TenantRecord>> {
		if !matches!(actor, Actor::DeploymentAdministrator) {
			return Err(Error::Forbidden);
		}
		let lease = self.runtime.store.orm_connection()?;
		let tenants = lease
			.handle()
			.atomic(async |tx| tenants::list(tx, page.offset).await)
			.await?;
		Ok(tenants.into_iter().map(Into::into).collect())
	}

	/// Changes the label of the caller's Tenant under `tenant.update`, through
	/// a browser Mapping, in the transaction that checks the authority.
	pub(crate) async fn relabel(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: LabelUpdate,
	) -> Result<TenantLabel> {
		validate(&input)?;
		aidash_domain::policy::identifier(&tenant)?;
		let (Actor::Subject(identity), Some(origin)) = (actor, origin) else {
			return Err(Error::Forbidden);
		};
		if identity.tenant != tenant {
			return Err(Error::Forbidden);
		}
		let mut access = NativeAccess::begin(&self.runtime.store, &identity).await?;
		let resource = Resource {
			tenant: tenant.clone(),
			kind: "tenant".into(),
			id: tenant.clone(),
			attributes: json!({}),
		};
		if let Err(error) = access.require(&resource, TENANT_UPDATE).await {
			return access.finish(Err(error)).await;
		}
		let result = async {
			let (tx, _) = access.tenant_transaction();
			let label = tenants::relabel(tx, &tenant, &input.label).await?;
			AdministrationActor::Identity {
				identity: origin.identity_id,
				mapping: origin.mapping_id,
			}
			.record(
				tx,
				AdministrationEvent {
					action: TENANT_UPDATE,
					identity: None,
					registration: None,
					mapping: None,
					tenant: Some(&tenant),
					subject: Some(&identity.subject),
					invitation: None,
				},
			)
			.await?;
			Ok::<_, Error>(label)
		}
		.await;
		let label = access.finish(result).await?;
		Ok(TenantLabel {
			name: tenant,
			label,
		})
	}
}

/// Whether `identity` may create Tenants now, for the dashboard to offer it.
pub(crate) fn creation_offered(f: &Federation, pool: Option<&str>, disabled: bool) -> bool {
	!disabled
		&& f.config.tenancy.tenant_creation
		&& f.config
			.dashboard_policy()
			.is_some_and(|policy| policy.unbound(pool))
}
