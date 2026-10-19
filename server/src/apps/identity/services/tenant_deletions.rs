//! Deleting and restoring Tenants (ADR 0060). A subject that its Tenant's own
//! policy allows `tenant.delete` deletes through its Mapping, and so may
//! restore; Deployment Administrators may always do both, and only they
//! restore a Tenant one of them deleted. Restoring cannot authenticate the
//! usual way, because every boundary of a deleted Tenant refuses: it uses only
//! the caller's Mapping in that Tenant and that Tenant's own frozen policy.
use crate::apps::identity::models::{
	AdministrationActor, AdministrationEvent, AuthorizationBundle, DashboardMapping, Tenant,
	TenantAdministrator, TenantDeletion,
};
use crate::apps::identity::oidc::{
	BrowserOrigin, csrf_allowed, required_config, session_from_headers,
};
use crate::apps::identity::repositories::tenant_deletions::{self, DeletionActor};
use crate::apps::identity::repositories::tenant_purge;
use crate::apps::identity::serializers::tenants::{TenantDeletionRequest, TenantDeletionView};
use crate::apps::identity::services::http_auth::bearer;
use crate::authorization::identity::{Actor, SubjectIdentity};
use crate::federation::Federation;
use crate::http::validate;
use crate::{Error, Result};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::tenancy::{TENANT_DELETE, deletion};
use http::{HeaderMap, Method};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::injectable;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct TenantDeletions {
	pub runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> TenantDeletions {
	TenantDeletions { runtime }
}

impl From<TenantDeletion> for TenantDeletionView {
	fn from(deletion: TenantDeletion) -> Self {
		Self {
			tenant: deletion.tenant,
			deleted_at: deletion.deleted_at,
			deleted_by_kind: deletion.deleted_by_kind,
			purge_after: deletion.purge_after,
			purged_at: deletion.purged_at,
		}
	}
}

/// Who deletes or restores a Tenant.
enum Steward {
	/// The bearer token (no origin) or a browser session holding a grant.
	DeploymentAdministrator(Option<BrowserOrigin>),
	/// A subject of the Tenant acting through its Mapping.
	TenantAdministrator(Arc<TenantAdministrator>),
}

impl Steward {
	fn actor(&self) -> DeletionActor {
		match self {
			Self::DeploymentAdministrator(origin) => DeletionActor {
				kind: deletion::Actor::DeploymentAdministrator,
				identity: origin.as_ref().map(|origin| origin.identity_id),
			},
			Self::TenantAdministrator(administrator) => DeletionActor {
				kind: deletion::Actor::TenantAdministrator,
				identity: Some(administrator.identity_id),
			},
		}
	}

	fn administration_actor(&self) -> AdministrationActor {
		match self {
			Self::DeploymentAdministrator(None) => {
				AdministrationActor::DeploymentAdministratorBearer
			}
			Self::DeploymentAdministrator(Some(origin)) => {
				AdministrationActor::DeploymentAdministrator {
					identity: origin.identity_id,
				}
			}
			Self::TenantAdministrator(administrator) => {
				AdministrationActor::TenantAdministrator(administrator.clone())
			}
		}
	}

	/// Locks the Tenant's policy exclusively, so no boundary interleaves, and
	/// checks this steward's authority over it. `deleted` reads the policy
	/// the Tenant was deleted with, for a restore.
	async fn authorize(
		&self,
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		deleted: bool,
	) -> Result<()> {
		match self {
			Self::DeploymentAdministrator(origin) => {
				if let Some(origin) = origin {
					origin.require_deployment_administrator(tx, true).await?;
				}
				if !Tenant::exists(tx, tenant).await? {
					return Err(Error::NotFound("tenant".into()));
				}
				if deleted {
					AuthorizationBundle::lock_frozen(tx, tenant, true).await?;
				} else {
					AuthorizationBundle::lock_snapshot(tx, tenant, true).await?;
				}
				Ok(())
			}
			Self::TenantAdministrator(administrator) => {
				let snapshot = if deleted {
					administrator.lock_deleted(tx).await?
				} else {
					administrator.lock(tx, true).await?
				};
				administrator
					.require(tx, &snapshot, TENANT_DELETE, tenant, json!({}))
					.await
			}
		}
	}

	/// A Tenant Administrator's denial is audited after its transaction rolled back.
	async fn finish<T>(&self, f: &Federation, result: Result<T>) -> Result<T> {
		if let (Self::TenantAdministrator(administrator), Err(Error::Forbidden)) = (self, &result) {
			administrator.record_denial(&f.store.pool).await?;
		}
		result
	}
}

impl TenantDeletions {
	fn tenant_administrator(
		&self,
		identity: SubjectIdentity,
		origin: &BrowserOrigin,
		tenant: &str,
	) -> Result<Arc<TenantAdministrator>> {
		let mapping = origin.mapping_id.ok_or(Error::Forbidden)?;
		// A Mapping acts only in its own Tenant (ADR 0060).
		if identity.tenant != tenant {
			return Err(Error::Forbidden);
		}
		Ok(Arc::new(TenantAdministrator::new(
			identity,
			origin.identity_id,
			mapping,
			self.runtime.config.node_id.clone(),
			self.runtime.config.dashboard_policy(),
		)?))
	}

	/// Records a Tenant Deletion. The Tenant stops at once: every boundary
	/// refuses it, its unfinished Runs stop and its pending Invitations are
	/// revoked. Its Plan Connections are then retired at OpenAI.
	pub(crate) async fn delete(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: TenantDeletionRequest,
	) -> Result<TenantDeletionView> {
		validate(&input)?;
		aidash_domain::policy::identifier(&tenant)?;
		deletion::confirm(&tenant, &input.confirmation)?;
		let steward = match (actor, origin) {
			(Actor::DeploymentAdministrator, origin) => Steward::DeploymentAdministrator(origin),
			(Actor::Subject(identity), Some(origin)) => {
				identity.check_binding(&self.runtime.store.pool).await?;
				Steward::TenantAdministrator(self.tenant_administrator(identity, &origin, &tenant)?)
			}
			_ => return Err(Error::Forbidden),
		};
		let f = &self.runtime;
		let grace_days = f.config.tenancy.deletion_grace_days;
		let result = f
			.store
			.pool
			.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
				steward.authorize(tx, &tenant, false).await?;
				let deletion =
					tenant_deletions::record(tx, &tenant, steward.actor(), grace_days).await?;
				steward
					.administration_actor()
					.record(tx, event(TENANT_DELETE, &tenant))
					.await?;
				Ok(deletion)
			})
			.await;
		let deletion = steward.finish(f, result).await?;
		retire_plan_connections(f, &tenant, &steward.administration_actor()).await;
		Ok(deletion.into())
	}

	/// Restores a deleted Tenant within its grace period. A Tenant
	/// Administrator names its Mapping in the deleted Tenant through the usual
	/// context selector; it is checked against that Tenant's frozen policy.
	pub(crate) async fn restore(
		&self,
		headers: HeaderMap,
		tenant: String,
	) -> Result<TenantDeletionView> {
		aidash_domain::policy::identifier(&tenant)?;
		let steward = self.restorer(&headers, &tenant).await?;
		let f = &self.runtime;
		let scope = TenancyScope::tenant(&tenant)?;
		// Retiring at OpenAI cannot be undone, so it waits until this caller
		// is known to restore a deleted Tenant. The check rolls back; the
		// restore below decides again under its locks and keeps that
		// decision, and a denial here is audited as one there would be.
		let mut check = f.store.pool.begin(&scope).await?;
		let checked = async {
			refuse_purged(&mut check, &tenant).await?;
			steward.authorize(&mut check, &tenant, true).await?;
			tenant_deletions::current_locked(&mut check, &tenant)
				.await?
				.ok_or_else(|| Error::Conflict("the Tenant is not deleted".into()))
		}
		.await;
		check.rollback().await?;
		steward.finish(f, checked).await?;
		// A retirement that failed at deletion is retried before the Tenant
		// works again: no Plan Connection outlives its Tenant's deletion, and
		// a failure leaves the Tenant deleted (ADR 0007).
		if let Some(service) = &f.store.plan_connections {
			service
				.retire_tenant(&tenant, &steward.administration_actor().decision_actor())
				.await?;
		}
		let result = f
			.store
			.pool
			.atomic(&scope, async |tx| {
				steward.authorize(tx, &tenant, true).await?;
				let deletion = tenant_deletions::current(tx, &tenant)
					.await?
					.ok_or_else(|| Error::Conflict("the Tenant is not deleted".into()))?;
				tenant_deletions::restore(tx, &deletion, steward.actor()).await?;
				steward
					.administration_actor()
					.record(tx, event("tenant.restore", &tenant))
					.await?;
				Ok(deletion)
			})
			.await;
		let deletion = steward.finish(f, result).await?;
		Ok(deletion.into())
	}

	/// Ends the grace period of a deleted Tenant now, so its data is purged at
	/// once instead of at the deadline (ADR 0061). Only a Deployment
	/// Administrator may: a Tenant Administrator who could skip the grace
	/// period could not be undone by anyone. The purge itself runs on the
	/// retention loop, and is started here without waiting for the next sweep.
	pub(crate) async fn purge_now(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<TenantDeletionView> {
		aidash_domain::policy::identifier(&tenant)?;
		if !matches!(actor, Actor::DeploymentAdministrator) {
			return Err(Error::Forbidden);
		}
		let steward = Steward::DeploymentAdministrator(origin);
		let f = &self.runtime;
		let result = f
			.store
			.pool
			.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
				refuse_purged(tx, &tenant).await?;
				steward.authorize(tx, &tenant, true).await?;
				let deletion = tenant_deletions::current_locked(tx, &tenant)
					.await?
					.ok_or_else(|| Error::Conflict("the Tenant is not deleted".into()))?;
				let deletion = tenant_purge::purge_now(tx, &deletion).await?;
				steward
					.administration_actor()
					.record(tx, event("tenant.purge", &tenant))
					.await?;
				Ok(deletion)
			})
			.await;
		let deletion = steward.finish(f, result).await?;
		let store = f.store.clone();
		let audit_days = f.config.tenancy.audit_retention_days;
		tokio::spawn(async move {
			if let Err(error) =
				crate::apps::identity::services::tenant_purge::sweep(&store, audit_days).await
			{
				tracing::warn!(%error, "the purge started by a Deployment Administrator failed");
			}
		});
		Ok(deletion.into())
	}

	/// The Deployment Administrator bearer token, a browser session in the
	/// `deployment_administrator` context, or a browser session naming its
	/// Mapping in the deleted Tenant. Subject Credentials never restore.
	async fn restorer(&self, headers: &HeaderMap, tenant: &str) -> Result<Steward> {
		let f = &self.runtime;
		let desktop = crate::apps::identity::repositories::desktop::access_token(headers).is_some();
		if !desktop && let Some(token) = bearer(headers) {
			return if crate::config::same_secret(token, &f.config.api_token) {
				Ok(Steward::DeploymentAdministrator(None))
			} else {
				Err(Error::Unauthorized)
			};
		}
		let config = required_config(f)?;
		let session = session_from_headers(f, headers).await?;
		if !csrf_allowed(config, headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let selector = headers
			.get("x-aidash-context")
			.and_then(|value| value.to_str().ok())
			.ok_or(Error::Forbidden)?;
		if selector == "deployment_administrator" {
			return Ok(Steward::DeploymentAdministrator(Some(
				BrowserOrigin::unmapped(config, &session),
			)));
		}
		let (named, id) = aidash_domain::identity::credentials::parse_mapping_selector(selector)
			.ok_or(Error::Forbidden)?;
		if named.as_str() != tenant {
			return Err(Error::Forbidden);
		}
		let mapping =
			f.store
				.pool
				.atomic(&TenancyScope::Tenant(named), async |tx| {
					DashboardMapping::find(tx, id).await
				})
				.await?
				.filter(|mapping| {
					mapping.identity_id() == session.identity_id()
						&& mapping.enabled && mapping.tenant == tenant
				})
				.ok_or(Error::Forbidden)?;
		let origin = BrowserOrigin::unmapped(config, &session);
		let origin = BrowserOrigin {
			mapping_id: Some(id),
			..origin
		};
		let identity = SubjectIdentity {
			http_session: Some(origin.session.clone()),
			credential_id: mapping.credential_id(),
			tenant: mapping.tenant,
			subject: mapping.subject,
		};
		identity.check_binding(&f.store.pool).await?;
		Ok(Steward::TenantAdministrator(
			self.tenant_administrator(identity, &origin, tenant)?,
		))
	}
}

/// Refuses a Tenant whose data was purged. Its policy went with its data, so
/// every later check of the Tenant would fail for that reason alone.
async fn refuse_purged(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<()> {
	if tenant_deletions::is_purged(tx, tenant).await? {
		return Err(Error::Conflict("tenant_purged".into()));
	}
	Ok(())
}

fn event<'a>(action: &'a str, tenant: &'a str) -> AdministrationEvent<'a> {
	AdministrationEvent {
		action,
		identity: None,
		registration: None,
		mapping: None,
		tenant: Some(tenant),
		// The event concerns the Tenant; who acted is in the actor.
		subject: None,
		invitation: None,
	}
}

/// Ends every Plan Connection of a deleted Tenant at OpenAI, which needs the
/// refresh tokens still held, so it happens now rather than at purge (ADR
/// 0007). The deletion already committed, so a failure is logged rather than
/// reported; a connection left behind cannot serve calls while the Tenant is
/// deleted, and a restore retires it before the Tenant works again.
async fn retire_plan_connections(f: &Federation, tenant: &str, actor: &AdministrationActor) {
	let Some(service) = &f.store.plan_connections else {
		return;
	};
	if service
		.retire_tenant(tenant, &actor.decision_actor())
		.await
		.is_err()
	{
		tracing::warn!("the Plan Connections of a deleted Tenant were not all retired");
	}
}
