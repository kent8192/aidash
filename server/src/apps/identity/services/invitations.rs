//! Invitations. Tenant Administrators issue them for their Tenant under
//! `invitation.*`, and Deployment Administrators for any Tenant, as people
//! administration that does not reach into Tenant content. Opening a link
//! spends nothing; requesting to join with it does.
use crate::apps::identity::models::{
	AdministrationActor, AuthorizationBundle, DashboardDeploymentAdministratorGrant,
	DashboardIdentity, DashboardMapping, DashboardRegistrationRequest, Tenant, TenantAdministrator,
	TenantInvitation, database_time,
};
use crate::apps::identity::oidc::{
	BrowserOrigin, csrf_allowed, digest, random_secret, required_config, session_from_headers,
};
use crate::apps::identity::repositories::invitations;
use crate::apps::identity::serializers::invitations::{
	InvitationLink, InvitationPreview, InvitationRequest, InvitationView, IssuedInvitation,
};
use crate::apps::identity::serializers::oidc::{Approval, Registration};
use crate::authorization::identity::{Actor, SubjectIdentity};
use crate::federation::Federation;
use crate::http::validate;
use crate::{Error, Result};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::tenancy::invitation;
use aidash_domain::identity::tenant_administration::{
	COLLECTION, INVITATION_CREATE, INVITATION_READ, INVITATION_REVOKE, REGISTRATION_APPROVE,
};
use http::{HeaderMap, Method};
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::injectable;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::json;
use std::collections::BTreeSet;
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
	DeploymentAdministrator(Option<BrowserOrigin>),
	TenantAdministrator(Arc<TenantAdministrator>),
}

impl Issuer {
	fn actor(&self) -> AdministrationActor {
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

	/// Locks the policy and checks the issuer's current authority.
	async fn authorize(
		&self,
		tx: &mut AtomicTransaction,
		tenant: &str,
		action: &str,
		id: &str,
	) -> Result<crate::authorization::Snapshot> {
		match self {
			Self::DeploymentAdministrator(origin) => {
				if !Tenant::exists(tx, tenant).await? {
					return Err(Error::NotFound("tenant".into()));
				}
				let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant, false).await?;
				if action == INVITATION_CREATE
					&& let Some(origin) = origin
				{
					origin.require_deployment_administrator(tx, true).await?;
				}
				Ok(snapshot)
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

struct JoinDecision<'a> {
	registration: Uuid,
	identity: Uuid,
	subject: &'a str,
	groups: &'a BTreeSet<String>,
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
			(Actor::DeploymentAdministrator, origin) => Ok(Issuer::DeploymentAdministrator(origin)),
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
						self.runtime.config.deployment.to_string(),
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
		let result = self
			.runtime
			.store
			.pool
			.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
				let snapshot = issuer
					.authorize(tx, &tenant, INVITATION_CREATE, COLLECTION)
					.await?;
				let join_directly = match (&issuer, input.join_directly) {
					(_, Some(false)) => false,
					(Issuer::TenantAdministrator(administrator), Some(true)) => {
						administrator
							.require(tx, &snapshot, REGISTRATION_APPROVE, COLLECTION, json!({}))
							.await?;
						true
					}
					(Issuer::TenantAdministrator(administrator), None) => {
						administrator
							.allowed(tx, &snapshot, REGISTRATION_APPROVE, COLLECTION, json!({}))
							.await?
					}
					(Issuer::DeploymentAdministrator(_), _) => true,
				};
				let groups = invitations::proposed_groups(&snapshot.bundle, input.groups.clone())?;
				invitations::issue(
					tx,
					&tenant,
					groups,
					lifetime,
					&issuer.actor(),
					join_directly,
					match &issuer {
						Issuer::DeploymentAdministrator(None) if join_directly => {
							Some(self.runtime.config.api_token.as_str())
						}
						_ => None,
					},
				)
				.await
			})
			.await;
		let issued = self.finish(&issuer, result).await?;
		Ok(IssuedInvitation {
			id: issued.invitation.id,
			link: invitation_link(&public_origin, &tenant, &issued.secret),
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
		let result = self
			.runtime
			.store
			.pool
			.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
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
		let result = self
			.runtime
			.store
			.pool
			.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
				issuer
					.authorize(tx, &tenant, INVITATION_REVOKE, &id.to_string())
					.await?;
				let revoked = invitations::revoke(tx, &tenant, id, &issuer.actor()).await?;
				Ok(view(revoked, database_time(tx).await?))
			})
			.await;
		self.finish(&issuer, result).await
	}

	/// The browser session of the External Identity presenting a link.
	/// Tenancy Rules do not cover identities with a Tenant Binding, so they
	/// cannot redeem. Redemption rechecks the session under lock.
	async fn holder(&self, headers: &HeaderMap) -> Result<BrowserOrigin> {
		self.require_enabled()?;
		let f = &self.runtime;
		let config = required_config(f)?;
		let session = session_from_headers(f, headers).await?;
		if !csrf_allowed(config, headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let identity = f
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::find(tx, session.identity_id()).await
			})
			.await?;
		if identity.disabled_at.is_some()
			|| !f
				.config
				.dashboard_policy()
				.is_some_and(|policy| policy.unrestricted(&identity.issuer))
		{
			return Err(Error::Forbidden);
		}
		Ok(BrowserOrigin::unmapped(config, &session))
	}

	/// Shows the inviting Tenant's label without spending the link.
	pub(crate) async fn inspect(
		&self,
		headers: HeaderMap,
		input: InvitationLink,
	) -> Result<InvitationPreview> {
		validate(&input)?;
		self.holder(&headers).await?;
		self.runtime
			.store
			.pool
			.atomic(&link_scope(&input.tenant)?, async |tx| {
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

	/// Reconstruct and lock the saved issuer's current authority, not its old session.
	async fn invitation_approver(
		&self,
		tx: &mut AtomicTransaction,
		invitation: &TenantInvitation,
		secret: &str,
		exclusive: bool,
		decision: &JoinDecision<'_>,
	) -> Result<Option<(AdministrationActor, crate::authorization::Snapshot)>> {
		let tenant = &invitation.tenant;
		if invitation.issued_by_identity == Some(decision.identity) {
			return Ok(None);
		}
		if invitation.issued_by_kind == "deployment_administrator" {
			// A revoked grant cannot approve and need not contend for a policy
			// change lock. A positive read is checked again under the grant lock.
			if let Some(identity) = invitation.issued_by_identity
				&& !DashboardDeploymentAdministratorGrant::enabled_for_identity(tx, identity)
					.await?
			{
				return Ok(None);
			}
			if invitation.issued_by_identity.is_none()
				&& invitation
					.issued_by_token_hash
					.as_deref()
					.is_none_or(|tag| {
						!invitations::bearer_matches(secret, &self.runtime.config.api_token, tag)
					}) {
				return Ok(None);
			}
			let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant, true).await?;
			let actor = match invitation.issued_by_identity {
				None => AdministrationActor::DeploymentAdministratorBearer,
				Some(identity) => {
					let Some((issuer, last_valid_at, disabled_at)) =
						lock_deployment_grant(tx, identity).await?
					else {
						return Ok(None);
					};
					let policy = self.runtime.config.dashboard_policy();
					if policy
						.as_ref()
						.is_none_or(|p| p.require_identity(&issuer).is_err())
						|| crate::authorization::identity::validate_local_status(
							policy.as_ref(),
							Some((issuer, last_valid_at, disabled_at)),
						)
						.is_err()
					{
						return Ok(None);
					}
					if let Some(authority) = policy.as_ref().and_then(|p| p.authority.as_ref())
						&& authority.validate_origin(identity).await.is_err()
					{
						return Ok(None);
					}
					AdministrationActor::DeploymentAdministrator { identity }
				}
			};
			return Ok(Some((actor, snapshot)));
		}
		if invitation.issued_by_kind != "tenant_administrator" {
			return Ok(None);
		}
		let Some(identity) = invitation.issued_by_identity else {
			return Ok(None);
		};
		let mappings = DashboardMapping::objects()
			.filter(DashboardMapping::field_identity_id().eq(identity))
			.filter(DashboardMapping::field_tenant().eq(tenant.clone()))
			.filter(DashboardMapping::field_enabled().eq(true))
			.order_by(&["id"])
			.all_with_db(tx)
			.await?;
		// Lock Mappings in ID order even if links were issued from different
		// Mappings; only the same External Identity can supply authority.
		let mut candidates = Vec::new();
		for mapping in mappings {
			let mapping_id = mapping.id;
			let credential_id = mapping.credential_id();
			let subject = SubjectIdentity {
				http_session: None,
				credential_id,
				tenant: tenant.clone(),
				subject: mapping.subject,
			};
			let administrator = match TenantAdministrator::new(
				subject,
				identity,
				mapping_id,
				self.runtime.config.deployment.to_string(),
				self.runtime.config.dashboard_policy(),
			) {
				Ok(administrator) => Arc::new(administrator),
				Err(Error::Forbidden | Error::Unauthorized) => continue,
				Err(error) => return Err(error),
			};
			let snapshot = match administrator
				.lock_change(tx, exclusive, REGISTRATION_APPROVE)
				.await
			{
				Ok(snapshot) => snapshot,
				Err(Error::Forbidden | Error::Unauthorized) => continue,
				Err(error) => return Err(error),
			};
			// A credential can also exist without a Mapping. Recheck this exact
			// Mapping under the share lock held by lock_change, even if it
			// disappeared before that lock was acquired.
			let current = DashboardMapping::objects()
				.filter(DashboardMapping::field_id().eq(mapping_id))
				.filter(DashboardMapping::field_identity_id().eq(identity))
				.filter(DashboardMapping::field_credential_id().eq(credential_id))
				.filter(DashboardMapping::field_tenant().eq(tenant.clone()))
				.filter(
					DashboardMapping::field_subject().eq(administrator.identity.subject.clone()),
				)
				.filter(DashboardMapping::field_enabled().eq(true))
				.all_with_db(tx)
				.await?;
			if current.is_empty() {
				continue;
			}
			candidates.push((administrator, snapshot));
		}
		// Take all issuer locks before the authorization audit cursor.
		candidates.sort_by_key(|(administrator, _)| {
			(
				Some(administrator.mapping_id) != invitation.issued_by_mapping,
				administrator.mapping_id,
			)
		});
		for (administrator, snapshot) in candidates {
			if administrator.allowed(tx, &snapshot, REGISTRATION_APPROVE, &decision.registration.to_string(),
				json!({"identity_id": decision.identity, "subject": decision.subject, "groups": decision.groups})).await? {
				return Ok(Some((AdministrationActor::TenantAdministrator(administrator), snapshot)));
			}
		}
		Ok(None)
	}

	/// Spends the link on a Registration Request addressed to its Tenant.
	pub(crate) async fn redeem(
		&self,
		headers: HeaderMap,
		input: InvitationLink,
	) -> Result<Registration> {
		validate(&input)?;
		let holder = self.holder(&headers).await?;
		// Admission may use another database connection. Evaluate it before the
		// write transaction, as the ordinary approval route does, but only apply
		// its verdict if the issuer still authorizes a direct join.
		let (needs_admission, issuer_subjects) = self
			.runtime
			.store
			.pool
			.atomic(&link_scope(&input.tenant)?, async |tx| {
				let Some(row) =
					invitations::usable(tx, &input.tenant, &input.secret, false).await?
				else {
					return Ok((false, Vec::new()));
				};
				if !row.join_directly || row.issued_by_kind != "tenant_administrator" {
					return Ok((false, Vec::new()));
				}
				let subjects = if let Some(identity) = row.issued_by_identity {
					DashboardMapping::objects()
						.filter(DashboardMapping::field_identity_id().eq(identity))
						.filter(DashboardMapping::field_tenant().eq(input.tenant.clone()))
						.filter(DashboardMapping::field_enabled().eq(true))
						.all_with_db(tx)
						.await?
						.into_iter()
						.map(|mapping| mapping.subject)
						.collect()
				} else {
					Vec::new()
				};
				Ok((true, subjects))
			})
			.await?;
		// Like the ordinary approval path, avoid raising an authority-change
		// flag for an issuer no policy can allow. The locked check detects a
		// policy that began allowing the action after this read.
		let exclusive = if issuer_subjects.is_empty() {
			false
		} else {
			let snapshot = crate::authorization::Authorization {
				pool: self.runtime.store.pool.clone(),
			}
			.snapshot(&input.tenant)
			.await?;
			issuer_subjects
				.iter()
				.any(|subject| snapshot.bundle.may_allow(subject, REGISTRATION_APPROVE))
		};
		let admission: Option<Result<()>> = if needs_admission {
			Some(
				aidash_application::usage::require(
					self.runtime.store.admission.as_ref(),
					&input.tenant,
					aidash_application::usage::Work::Member,
				)
				.await
				.map_err(Into::into),
			)
		} else {
			None
		};
		let admission_allowed = match admission {
			Some(Ok(())) | None => true,
			Some(Err(error)) if approval_refusal(&error) => false,
			Some(Err(error)) => return Err(error),
		};
		let registration = self
			.runtime
			.store
			.pool
			.atomic(&link_scope(&input.tenant)?, async |tx| {
				// Read the immutable issuer first. Approval takes policy and issuer
				// authority locks before the Invitation, holder identity and request,
				// matching the ordinary approval path and avoiding inverted locks.
				let preview = invitations::usable(tx, &input.tenant, &input.secret, false)
					.await?
					.ok_or_else(invalid_link)?;
				let identity = holder.session.identity_id;
				let registration_id = Uuid::new_v4();
				let groups = invitations::proposed(&preview);
				// Reuse the one previously mapped subject, just as manual Rejoin
				// does. Ambiguous historical subjects require a manual decision.
				let previous = DashboardMapping::objects()
					.filter(DashboardMapping::field_identity_id().eq(identity))
					.filter(DashboardMapping::field_tenant().eq(input.tenant.clone()))
					.filter(DashboardMapping::field_enabled().eq(false))
					.order_by(&["id"])
					.all_with_db(tx)
					.await?;
				let subject = previous
					.first()
					.map(|m| m.subject.clone())
					.unwrap_or_else(|| format!("user-{registration_id}"));
				let decision = JoinDecision {
					registration: registration_id,
					identity,
					subject: &subject,
					groups: &groups,
				};
				let approver = if preview.join_directly && previous.len() <= 1 && admission_allowed
				{
					// A changed policy or lost authority refuses only the optional
					// approval. Roll back its locks and audit before opening the request.
					match tx
						.atomic(async |tx| {
							self.invitation_approver(
								tx,
								&preview,
								&input.secret,
								exclusive,
								&decision,
							)
							.await
						})
						.await
					{
						Ok(approver) => approver,
						Err(error) if approval_refusal(&error) => None,
						Err(error) => return Err(error),
					}
				} else {
					None
				};
				let invitation = invitations::usable(tx, &input.tenant, &input.secret, true)
					.await?
					.ok_or_else(invalid_link)?;
				let mut registration =
					invitations::redeem(tx, invitation, &holder.session, registration_id).await?;
				if let Some((actor, snapshot)) = approver {
					// A direct join is bound by the current Assignable Groups for
					// every issuer, including Deployment Administrators.
					let assignable =
						aidash_domain::identity::tenant_administration::assignable_groups(
							&snapshot.bundle,
						);
					let current_previous = DashboardMapping::objects()
						.filter(DashboardMapping::field_identity_id().eq(identity))
						.filter(DashboardMapping::field_tenant().eq(input.tenant.clone()))
						.filter(DashboardMapping::field_enabled().eq(false))
						.order_by(&["id"])
						.all_with_db(tx)
						.await?;
					if !groups.is_subset(&assignable)
						|| current_previous.len() != previous.len()
						|| current_previous.first().map(|m| m.subject.as_str())
							!= previous.first().map(|m| m.subject.as_str())
					{
						return Ok(registration);
					}
					// The request and spent link live outside this savepoint. Any
					// approval refusal rolls back its partial effects and leaves the
					// request pending; storage failures still abort the transaction.
					let approved: Result<_> = tx
						.atomic(async |tx| {
							DashboardRegistrationRequest::approve_in(
								tx,
								registration.id,
								Approval {
									tenant: input.tenant.clone(),
									subject,
									groups: Some(groups.clone()),
								},
								groups,
								&actor,
								digest(&random_secret()),
								self.runtime.config.dashboard_policy(),
								true,
								Some(snapshot),
							)
							.await
						})
						.await;
					match approved {
						Ok(_) => {}
						Err(error) if approval_refusal(&error) => return Ok(registration),
						Err(error) => return Err(error),
					}
					registration = DashboardRegistrationRequest::objects()
						.filter(DashboardRegistrationRequest::field_id().eq(registration.id))
						.all_with_db(tx)
						.await?
						.pop()
						.ok_or_else(invalid_link)?;
				}
				Ok(registration)
			})
			.await?;
		Ok(registration.contract())
	}
}

/// Only expected approval refusals fall back; database and transaction
/// failures must remain visible so a broken savepoint cannot commit partial work.
fn approval_refusal(error: &Error) -> bool {
	matches!(
		error,
		Error::Invalid(_)
			| Error::Conflict(_)
			| Error::NotFound(_)
			| Error::Unauthorized
			| Error::Forbidden
			| Error::Unprocessable(_)
			| Error::Refused(_)
			| Error::TenantDeleted
			| Error::NodeLimitReached
			| Error::PolicyBusy { .. }
			| Error::IdentityStatusUnavailable
	)
}

/// Grant mutations take exclusive locks in this same identity-then-grant
/// order. Shared leases also coexist with a browser administrator's boundary.
async fn lock_deployment_grant(
	tx: &mut AtomicTransaction,
	identity: Uuid,
) -> Result<
	Option<(
		String,
		Option<chrono::DateTime<chrono::Utc>>,
		Option<chrono::DateTime<chrono::Utc>>,
	)>,
> {
	let sql = Query::select()
		.columns(["issuer", "last_valid_at", "disabled_at"].map(Alias::new))
		.from(Alias::new("dashboard_identities"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.lock(LockType::Share)
		.to_string(PostgresQueryBuilder);
	let person = crate::database::native::query_as(&sql)
		.columns(&["issuer", "last_valid_at", "disabled_at"])
		.bind(identity)
		.fetch_optional(&mut *tx)
		.await?;
	let sql = Query::select()
		.column(Alias::new("enabled"))
		.from(Alias::new("dashboard_deployment_administrator_grants"))
		.and_where(Expr::col(Alias::new("identity_id")).eq(Expr::cust("$1")))
		.lock(LockType::Share)
		.to_string(PostgresQueryBuilder);
	let enabled: Option<bool> = crate::database::native::query_scalar(&sql)
		.bind(identity)
		.scalar_optional(tx)
		.await?;
	Ok(if enabled == Some(true) { person } else { None })
}

/// One answer for every unusable link, so it reveals nothing about why.
fn invalid_link() -> Error {
	Error::NotFound("invalid link".into())
}

/// A link names its Tenant, so it is read and spent in that Tenant's scope.
/// A name no Tenant could have is as unusable as any other link.
fn link_scope(tenant: &str) -> Result<TenancyScope> {
	TenancyScope::tenant(tenant).map_err(|_| invalid_link())
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

/// What a Tenant name keeps verbatim in a link's path segment: RFC 3986's
/// unreserved characters. A valid name may still hold `#`, `?` or `/`.
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
	.remove(b'-')
	.remove(b'.')
	.remove(b'_')
	.remove(b'~');

/// The dashboard link of an Invitation; its secret travels in the fragment.
fn invitation_link(public_origin: &str, tenant: &str, secret: &str) -> String {
	let tenant = percent_encoding::utf8_percent_encode(tenant, PATH_SEGMENT);
	format!("{public_origin}/invitations/{tenant}#{secret}")
}

#[cfg(test)]
mod tests {
	use super::invitation_link;

	#[test]
	fn an_invitation_link_encodes_its_tenant_as_one_path_segment() {
		assert_eq!(
			invitation_link("https://aidash.example", "acme-east_1.x~y", "s3cret"),
			"https://aidash.example/invitations/acme-east_1.x~y#s3cret"
		);
		assert_eq!(
			invitation_link("https://aidash.example", "customer#east?a/b%", "s3cret"),
			"https://aidash.example/invitations/customer%23east%3Fa%2Fb%25#s3cret"
		);
	}
}
