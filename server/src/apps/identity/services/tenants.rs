//! The Tenant creation API and Tenant labels. One API creates every Tenant:
//! a signed-in External Identity where the Tenancy Rules allow it, and a
//! Deployment Administrator always. Subject Credentials, agents and peers never.
use crate::apps::identity::models::{
	AdministrationActor, AdministrationEvent, DashboardIdentity, Tenant, TenantDeletion,
	database_time,
};
use crate::apps::identity::oidc::{
	BrowserOrigin, csrf_allowed, required_config, session_from_headers,
};
use crate::apps::identity::repositories::tenants::{self, Created, Creator};
use crate::apps::identity::repositories::{personal_tenant, tenant_deletions, tenant_purge};
use crate::apps::identity::serializers::tenants::{
	CreatedTenant, LabelUpdate, PurgeProgress, TenantCreation, TenantLabel, TenantRecord,
};
use crate::apps::identity::services::http_auth::bearer;
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::authorization::{access::NativeAccess, identity::Actor};
use crate::federation::Federation;
use crate::http::validate;
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
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

fn record(
	tenant: Tenant,
	deletion: Option<TenantDeletion>,
	now: chrono::DateTime<chrono::Utc>,
) -> TenantRecord {
	let restorable = deletion.as_ref().is_some_and(|deletion| {
		deletion.rule().is_ok_and(|rule| {
			aidash_domain::identity::tenancy::deletion::restorable(
				&rule,
				aidash_domain::identity::tenancy::deletion::Actor::DeploymentAdministrator,
				now,
			)
		})
	});
	let purgeable = deletion
		.as_ref()
		.is_some_and(|deletion| deletion.purged_at.is_none());
	let purge = deletion
		.as_ref()
		.filter(|deletion| {
			deletion.purged_at.is_none()
				&& (deletion.purge_step.is_some() || deletion.purge_failures > 0)
		})
		.map(|deletion| PurgeProgress {
			step: deletion.purge_step.clone(),
			failures: u32::try_from(deletion.purge_failures).unwrap_or(u32::MAX),
			error: deletion.purge_error.clone(),
			failed_at: deletion.purge_failed_at,
			stalled: tenant_purge::stalled(deletion),
		});
	TenantRecord {
		name: tenant.name,
		label: tenant.label,
		personal: tenant.personal,
		created_at: tenant.created_at,
		created_by_kind: tenant.created_by_kind,
		deletion: deletion.map(Into::into),
		restorable,
		purgeable,
		purge,
	}
}

impl Tenants {
	/// The Deployment Administrator bearer token, a browser session in the
	/// `deployment_administrator` context, or any other browser session as the External
	/// Identity itself. A session needs no Mapping. Every other bearer token,
	/// such as a Subject Credential, is refused. A browser session's origin is
	/// returned with it: the creation transaction rechecks the session, and a
	/// Deployment Administrator's grant, under lock.
	async fn creator(&self, headers: &HeaderMap) -> Result<(Creator, Option<BrowserOrigin>)> {
		let f = &self.runtime;
		let desktop = crate::apps::identity::repositories::desktop::access_token(headers).is_some();
		if !desktop && let Some(token) = bearer(headers) {
			return if crate::config::same_secret(token, &f.config.api_token) {
				Ok((Creator::DeploymentAdministrator(None), None))
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
			return Ok((
				Creator::DeploymentAdministrator(Some(identity)),
				Some(BrowserOrigin::unmapped(config, &session)),
			));
		}
		Ok((
			Creator::Identity(identity),
			Some(BrowserOrigin::unmapped(config, &session)),
		))
	}

	pub(crate) async fn create(
		&self,
		headers: HeaderMap,
		input: TenantCreation,
	) -> Result<CreatedTenant> {
		validate(&input)?;
		let label = tenancy::label(&input.label)?;
		let (creator, origin) = self.creator(&headers).await?;
		let f = self.runtime.clone();
		let scope = TenancyScope::Deployment(DeploymentScopeReason::TenantCreation);
		let created = f
			.store
			.pool
			.atomic(&scope, async |tx| {
				// As at the authenticated write boundary, the identity, grant and
				// session stay share-locked until the Tenant exists: a revocation
				// committing meanwhile waits, or is seen here.
				if let (Creator::DeploymentAdministrator(_), Some(origin)) = (creator, &origin) {
					origin.require_deployment_administrator(tx, true).await?;
				}
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
					if account.disabled_at.is_some()
						|| !f
							.config
							.dashboard_policy()
							.is_some_and(|policy| policy.unrestricted(&account.issuer))
					{
						return Err(Error::Forbidden);
					}
					// The session, after the identity as at the write boundary:
					// a logout committing meanwhile waits, or is seen here.
					if let Some(origin) = &origin {
						origin.session.current(tx, true).await?;
					}
					let now = database_time(tx).await?;
					let (recent, total) = tenants::creations(tx, identity, now).await?;
					tenancy::check_creation(&f.config.tenancy, now, &recent, total)
						.map_err(Error::TenantCreation)?;
				}
				tenants::create(tx, &f.config.node_id, &label, false, creator).await
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
		let scope = TenancyScope::Deployment(DeploymentScopeReason::DeploymentAdministratorList);
		let (tenants, mut deletions, now) = self
			.runtime
			.store
			.pool
			.atomic(&scope, async |tx| {
				let tenants = tenants::list(tx, page.offset).await?;
				let deletions = tenant_deletions::current_of(
					tx,
					tenants.iter().map(|tenant| tenant.name.clone()).collect(),
				)
				.await?;
				Ok((tenants, deletions, database_time(tx).await?))
			})
			.await?;
		Ok(tenants
			.into_iter()
			.map(|tenant| {
				let deletion = deletions.remove(&tenant.name);
				record(tenant, deletion, now)
			})
			.collect())
	}

	/// Creates a new Personal Tenant for a signed-in External Identity whose
	/// Personal Tenant was deleted, where the Tenancy Rules still provide
	/// Personal Tenants. The dashboard first offers to restore the deleted one.
	pub(crate) async fn recreate_personal(&self, headers: HeaderMap) -> Result<CreatedTenant> {
		let f = &self.runtime;
		let config = required_config(f)?;
		let session = session_from_headers(f, &headers).await?;
		// A desktop session is accepted as by ordinary Tenant creation;
		// `csrf_allowed` requires its access token.
		if !csrf_allowed(config, &headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let origin = BrowserOrigin::unmapped(config, &session);
		let identity = session.identity_id();
		let account = f
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::find(tx, identity).await
			})
			.await?;
		if account.disabled_at.is_some()
			|| !f
				.config
				.dashboard_policy()
				.is_some_and(|policy| policy.provisions_personal(&account.issuer))
		{
			return Err(Error::Forbidden);
		}
		Ok(
			personal_tenant::recreate(&f.store.pool, &f.config.node_id, identity, &origin.session)
				.await?
				.into(),
		)
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
					// The event concerns the Tenant; who acted is in the actor.
					subject: None,
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
pub(crate) fn creation_offered(f: &Federation, issuer: &str, disabled: bool) -> bool {
	!disabled
		&& f.config.tenancy.tenant_creation
		&& f.config
			.dashboard_policy()
			.is_some_and(|policy| policy.unrestricted(issuer))
}
