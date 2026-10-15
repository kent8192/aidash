//! Invitations: single-use, expiring links that let whoever opens them make a
//! Registration Request addressed to the inviting Tenant. Only a digest of the
//! secret is stored; the secret is shown once, when the Invitation is issued.
use crate::apps::identity::models::{
	AdministrationActor, AdministrationEvent, DashboardMapping, DashboardRegistrationRequest,
	TenantInvitation, database_time,
};
use crate::apps::identity::services::oidc::{digest, random_secret};
use crate::{Error, Result};
use aidash_domain::identity::tenancy::invitation::MAX_ACTIVE;
use aidash_domain::identity::tenant_administration::{
	INVITATION_CREATE, INVITATION_REVOKE, assignable_groups,
};
use aidash_domain::policy::PolicyBundle;
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::{AtomicTransaction, Json, Model};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The groups an Invitation proposes: those the issuer named, which must be
/// Assignable Groups, or `members` when it is one.
pub(crate) fn proposed_groups(
	bundle: &PolicyBundle,
	requested: Option<BTreeSet<String>>,
) -> Result<BTreeSet<String>> {
	let assignable = assignable_groups(bundle);
	match requested {
		Some(groups) => {
			if let Some(group) = groups.iter().find(|group| !assignable.contains(*group)) {
				return Err(Error::Invalid(format!("not an Assignable Group: {group}")));
			}
			Ok(groups)
		}
		None => Ok(assignable
			.into_iter()
			.filter(|group| group == aidash_domain::identity::tenancy::MEMBERS)
			.collect()),
	}
}

fn groups_of(invitation: &TenantInvitation) -> BTreeSet<String> {
	serde_json::from_value(invitation.groups.0.clone()).unwrap_or_default()
}

/// What an issued Invitation shows its issuer once.
pub(crate) struct Issued {
	pub invitation: TenantInvitation,
	pub groups: BTreeSet<String>,
	pub secret: String,
}

pub(crate) async fn issue(
	tx: &mut AtomicTransaction,
	tenant: &str,
	groups: BTreeSet<String>,
	lifetime: Duration,
	issuer: &AdministrationActor,
) -> Result<Issued> {
	let now = database_time(tx).await?;
	let active = TenantInvitation::objects()
		.filter(TenantInvitation::field_tenant().eq(tenant.to_owned()))
		.filter(TenantInvitation::field_revoked_at().is_null())
		.filter(TenantInvitation::field_redeemed_at().is_null())
		.filter(TenantInvitation::field_expires_at().gt(now))
		.count_with_db(tx)
		.await?;
	if active >= MAX_ACTIVE {
		return Err(Error::Conflict(format!(
			"a Tenant may have at most {MAX_ACTIVE} active Invitations"
		)));
	}
	let secret = random_secret();
	let (kind, identity) = issuer.kind_and_identity();
	let invitation = TenantInvitation::build()
		.id(Uuid::new_v4())
		.tenant(tenant.to_owned())
		.token_hash(digest(&secret))
		.groups(Json(serde_json::to_value(&groups)?))
		.created_at(now)
		.expires_at(now + lifetime)
		.issued_by_kind(kind.to_owned())
		.issued_by_identity(identity)
		.revoked_at(None)
		.redeemed_at(None)
		.redeemed_by(None)
		.registration_id(None)
		.finish();
	TenantInvitation::objects()
		.create_with_conn(tx, &invitation)
		.await?;
	issuer
		.record(
			tx,
			AdministrationEvent {
				action: INVITATION_CREATE,
				identity: None,
				registration: None,
				mapping: None,
				tenant: Some(tenant),
				subject: None,
				invitation: Some(invitation.id),
			},
		)
		.await?;
	Ok(Issued {
		invitation,
		groups,
		secret,
	})
}

/// The Tenant's Invitations, newest first.
pub(crate) async fn list(
	tx: &mut AtomicTransaction,
	tenant: &str,
) -> Result<Vec<TenantInvitation>> {
	Ok(TenantInvitation::objects()
		.filter(TenantInvitation::field_tenant().eq(tenant.to_owned()))
		.order_by(&["-created_at", "id"])
		.limit(200)
		.all_with_db(tx)
		.await?)
}

pub(crate) async fn revoke(
	tx: &mut AtomicTransaction,
	tenant: &str,
	id: Uuid,
	actor: &AdministrationActor,
) -> Result<TenantInvitation> {
	let mut invitation = TenantInvitation::objects()
		.filter(TenantInvitation::field_id().eq(id))
		.filter(TenantInvitation::field_tenant().eq(tenant.to_owned()))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("invitation".into()))?;
	if invitation.redeemed_at.is_some() || invitation.revoked_at.is_some() {
		return Err(Error::Conflict(
			"the Invitation was already used or revoked".into(),
		));
	}
	invitation.revoked_at = Some(database_time(tx).await?);
	TenantInvitation::objects()
		.update_with_conn(tx, &invitation)
		.await?;
	actor
		.record(
			tx,
			AdministrationEvent {
				action: INVITATION_REVOKE,
				identity: None,
				registration: None,
				mapping: None,
				tenant: Some(tenant),
				subject: None,
				invitation: Some(id),
			},
		)
		.await?;
	Ok(invitation)
}

/// The usable Invitation of `tenant` with this secret, locked when `lock`.
/// Expired, revoked and spent links are all simply absent.
pub(crate) async fn usable(
	tx: &mut AtomicTransaction,
	tenant: &str,
	secret: &str,
	lock: bool,
) -> Result<Option<TenantInvitation>> {
	let now = database_time(tx).await?;
	let query = TenantInvitation::objects()
		.filter(TenantInvitation::field_tenant().eq(tenant.to_owned()))
		.filter(TenantInvitation::field_token_hash().eq(digest(secret)))
		.filter(TenantInvitation::field_revoked_at().is_null())
		.filter(TenantInvitation::field_redeemed_at().is_null())
		.filter(TenantInvitation::field_expires_at().gt(now));
	let mut rows = if lock {
		query
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
	} else {
		query.all_with_db(tx).await?
	};
	Ok(rows.pop())
}

/// Spends `invitation` on a new Registration Request addressed to its Tenant.
/// An identity already mapped into the Tenant, or already waiting for it,
/// leaves the link unspent.
pub(crate) async fn redeem(
	tx: &mut AtomicTransaction,
	mut invitation: TenantInvitation,
	identity: Uuid,
) -> Result<DashboardRegistrationRequest> {
	if DashboardMapping::objects()
		.filter(DashboardMapping::field_identity_id().eq(identity))
		.filter(DashboardMapping::field_tenant().eq(invitation.tenant.clone()))
		.filter(DashboardMapping::field_enabled().eq(true))
		.exists_with_db(tx)
		.await?
	{
		return Err(Error::Conflict("already a member of this Tenant".into()));
	}
	let registration = DashboardRegistrationRequest::open_addressed(
		tx,
		identity,
		&invitation.tenant,
		Some(invitation.id),
	)
	.await?;
	invitation.redeemed_at = Some(registration.created_at);
	invitation.redeemed_by = Some(identity);
	invitation.registration_id = Some(registration.id);
	TenantInvitation::objects()
		.update_with_conn(tx, &invitation)
		.await?;
	AdministrationActor::Identity {
		identity,
		mapping: None,
	}
	.record(
		tx,
		AdministrationEvent {
			action: "invitation.redeem",
			identity: Some(identity),
			registration: Some(registration.id),
			mapping: None,
			tenant: Some(&invitation.tenant),
			subject: None,
			invitation: Some(invitation.id),
		},
	)
	.await?;
	Ok(registration)
}

/// The groups `invitation` proposed, for pre-filling its approval.
pub(crate) fn proposed(invitation: &TenantInvitation) -> BTreeSet<String> {
	groups_of(invitation)
}

/// The state an issuer sees for each Invitation.
pub(crate) fn state(invitation: &TenantInvitation, now: DateTime<Utc>) -> &'static str {
	if invitation.redeemed_at.is_some() {
		"redeemed"
	} else if invitation.revoked_at.is_some() {
		"revoked"
	} else if invitation.expires_at <= now {
		"expired"
	} else {
		"active"
	}
}
