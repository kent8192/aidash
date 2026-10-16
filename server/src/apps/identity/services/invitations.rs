//! Invitations. Tenant Administrators issue them for their Tenant under
//! `invitation.*`, and Deployment Administrators for any Tenant, as people
//! administration that does not reach into Tenant content. Opening a link
//! spends nothing; requesting to join with it does.
use crate::apps::identity::models::{
	AdministrationActor, AuthorizationBundle, DashboardIdentity, Tenant, TenantAdministrator,
	TenantInvitation, database_time,
};
use crate::apps::identity::oidc::{
	BrowserOrigin, csrf_allowed, required_config, session_from_headers,
};
use crate::apps::identity::repositories::invitations;
use crate::apps::identity::serializers::invitations::{
	InvitationLink, InvitationPreview, InvitationRequest, InvitationView, IssuedInvitation,
};
use crate::apps::identity::serializers::oidc::Registration;
use crate::authorization::identity::Actor;
use crate::federation::Federation;
use crate::http::validate;
use crate::{Error, Result};
use aidash_domain::identity::tenancy::invitation;
use aidash_domain::identity::tenant_administration::{
	COLLECTION, INVITATION_CREATE, INVITATION_READ, INVITATION_REVOKE,
};
use http::{HeaderMap, Method};
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::injectable;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct Invitations {
	pub runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> Invitations {
	Invitations { runtime }
}

/// Who administers a Tenant's Invitations.
enum Issuer {
	DeploymentAdministrator(Option<Uuid>),
	TenantAdministrator(Arc<TenantAdministrator>),
}

impl Issuer {
	fn actor(&self) -> AdministrationActor {
		match self {
			Self::DeploymentAdministrator(None) => {
				AdministrationActor::DeploymentAdministratorBearer
			}
			Self::DeploymentAdministrator(Some(identity)) => {
				AdministrationActor::DeploymentAdministrator {
					identity: *identity,
				}
			}
			Self::TenantAdministrator(administrator) => {
				AdministrationActor::TenantAdministrator(administrator.clone())
			}
		}
	}

	/// Locks the Tenant's policy and checks `action` for a Tenant Administrator.
	async fn authorize(
		&self,
		tx: &mut AtomicTransaction,
		tenant: &str,
		action: &str,
		id: &str,
	) -> Result<crate::authorization::Snapshot> {
		match self {
			Self::DeploymentAdministrator(_) => {
				if !Tenant::exists(tx, tenant).await? {
					return Err(Error::NotFound("tenant".into()));
				}
				AuthorizationBundle::lock_snapshot(tx, tenant, false).await
			}
			Self::TenantAdministrator(administrator) => {
				let snapshot = administrator.lock(tx, false).await?;
				administrator
					.require(tx, &snapshot, action, id, json!({}))
					.await?;
				Ok(snapshot)
			}
		}
	}
}

impl Invitations {
	async fn issuer(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: &str,
	) -> Result<Issuer> {
		aidash_domain::policy::identifier(tenant)?;
		match (actor, origin) {
			(Actor::DeploymentAdministrator, origin) => Ok(Issuer::DeploymentAdministrator(
				origin.map(|origin| origin.identity_id),
			)),
			(Actor::Subject(identity), Some(origin)) => {
				let mapping = origin.mapping_id.ok_or(Error::Forbidden)?;
				if identity.tenant != tenant {
					return Err(Error::Forbidden);
				}
				identity.check_binding(&self.runtime.store.pool).await?;
				Ok(Issuer::TenantAdministrator(Arc::new(
					TenantAdministrator::new(
						identity,
						origin.identity_id,
						mapping,
						self.runtime.config.node_id.clone(),
						self.runtime.config.dashboard_policy(),
					)?,
				)))
			}
			_ => Err(Error::Forbidden),
		}
	}

	/// Audits a Tenant Administrator's denial after its transaction rolled back.
	async fn finish<T>(&self, issuer: &Issuer, result: Result<T>) -> Result<T> {
		if let (Issuer::TenantAdministrator(administrator), Err(Error::Forbidden)) =
			(issuer, &result)
		{
			administrator
				.record_denial(&self.runtime.store.pool)
				.await?;
		}
		result
	}

	fn require_enabled(&self) -> Result<()> {
		if self.runtime.config.tenancy.invitations {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}

	pub(crate) async fn issue(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: InvitationRequest,
	) -> Result<IssuedInvitation> {
		validate(&input)?;
		self.require_enabled()?;
		let lifetime = invitation::lifetime(input.expires_in_seconds)?;
		let public_origin = required_config(&self.runtime)?.public_origin.to_owned();
		let issuer = self.issuer(actor, origin, &tenant).await?;
		let lease = self.runtime.store.orm_connection()?;
		let result = lease
			.handle()
			.atomic(async |tx| {
				let snapshot = issuer
					.authorize(tx, &tenant, INVITATION_CREATE, COLLECTION)
					.await?;
				let groups = invitations::proposed_groups(&snapshot.bundle, input.groups.clone())?;
				invitations::issue(tx, &tenant, groups, lifetime, &issuer.actor()).await
			})
			.await;
		let issued = self.finish(&issuer, result).await?;
		Ok(IssuedInvitation {
			id: issued.invitation.id,
			link: format!("{public_origin}/invitations/{tenant}#{}", issued.secret),
			tenant,
			secret: issued.secret,
			groups: issued.groups,
			expires_at: issued.invitation.expires_at,
		})
	}

	pub(crate) async fn list(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Vec<InvitationView>> {
		let issuer = self.issuer(actor, origin, &tenant).await?;
		let lease = self.runtime.store.orm_connection()?;
		let result = lease
			.handle()
			.atomic(async |tx| {
				issuer
					.authorize(tx, &tenant, INVITATION_READ, COLLECTION)
					.await?;
				let now = database_time(tx).await?;
				Ok(invitations::list(tx, &tenant)
					.await?
					.into_iter()
					.map(|row| view(row, now))
					.collect())
			})
			.await;
		self.finish(&issuer, result).await
	}

	pub(crate) async fn revoke(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
	) -> Result<InvitationView> {
		let issuer = self.issuer(actor, origin, &tenant).await?;
		let lease = self.runtime.store.orm_connection()?;
		let result = lease
			.handle()
			.atomic(async |tx| {
				issuer
					.authorize(tx, &tenant, INVITATION_REVOKE, &id.to_string())
					.await?;
				let revoked = invitations::revoke(tx, &tenant, id, &issuer.actor()).await?;
				Ok(view(revoked, database_time(tx).await?))
			})
			.await;
		self.finish(&issuer, result).await
	}

	/// The signed-in External Identity presenting a link. Tenancy Rules do not
	/// cover identities with a Tenant Binding, so they cannot redeem.
	async fn holder(&self, headers: &HeaderMap) -> Result<Uuid> {
		self.require_enabled()?;
		let f = &self.runtime;
		let config = required_config(f)?;
		let session = session_from_headers(f, headers).await?;
		if !csrf_allowed(config, headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let lease = f.store.orm_connection()?;
		let identity = DashboardIdentity::find(&mut lease.handle(), session.identity_id()).await?;
		let pool = (!identity.gcip_tenant.is_empty()).then_some(identity.gcip_tenant.as_str());
		if identity.disabled_at.is_some()
			|| !f
				.config
				.dashboard_policy()
				.is_some_and(|policy| policy.unbound(pool))
		{
			return Err(Error::Forbidden);
		}
		Ok(identity.id)
	}

	/// Shows the inviting Tenant's label without spending the link.
	pub(crate) async fn inspect(
		&self,
		headers: HeaderMap,
		input: InvitationLink,
	) -> Result<InvitationPreview> {
		validate(&input)?;
		self.holder(&headers).await?;
		let lease = self.runtime.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				invitations::usable(tx, &input.tenant, &input.secret, false)
					.await?
					.ok_or_else(invalid_link)?;
				let label = Tenant::objects()
					.filter(Tenant::field_name().eq(input.tenant.clone()))
					.all_with_db(tx)
					.await?
					.pop()
					.and_then(|tenant| tenant.label);
				Ok(InvitationPreview {
					tenant: input.tenant.clone(),
					label,
				})
			})
			.await
	}

	/// Spends the link on a Registration Request addressed to its Tenant.
	pub(crate) async fn redeem(
		&self,
		headers: HeaderMap,
		input: InvitationLink,
	) -> Result<Registration> {
		validate(&input)?;
		let identity = self.holder(&headers).await?;
		let lease = self.runtime.store.orm_connection()?;
		let registration = lease
			.handle()
			.atomic(async |tx| {
				let invitation = invitations::usable(tx, &input.tenant, &input.secret, true)
					.await?
					.ok_or_else(invalid_link)?;
				invitations::redeem(tx, invitation, identity).await
			})
			.await?;
		Ok(registration.contract())
	}
}

/// One answer for every unusable link, so it reveals nothing about why.
fn invalid_link() -> Error {
	Error::NotFound("invalid link".into())
}

fn view(row: TenantInvitation, now: chrono::DateTime<chrono::Utc>) -> InvitationView {
	InvitationView {
		state: invitations::state(&row, now).to_owned(),
		groups: invitations::proposed(&row),
		id: row.id,
		created_at: row.created_at,
		expires_at: row.expires_at,
		registration_id: row.registration_id,
	}
}
