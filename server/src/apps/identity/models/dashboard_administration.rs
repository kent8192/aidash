//! Transactional dashboard registration and authority changes.

use super::tenant_administration::AdministrationEvent;
use super::{
	AdministrationActor, AuthorizationBundle, AuthorizationCredential,
	DashboardDeploymentAdministratorGrant, DashboardIdentity, DashboardMapping,
	DashboardRegistrationRequest, DashboardSession, PersonalTenantLink, TenantAdministrator,
};
use crate::apps::identity::serializers::oidc::{
	AdminMapping, Approval, ApprovedMapping, DeploymentAdministratorGrant, IdentityView,
	Registration,
};
use crate::apps::identity::services::dashboard_rules;
use crate::authorization::Snapshot;
use crate::{Error, Result};
use aidash_domain::identity::tenant_administration;
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::query::FieldAssignment;
use reinhardt::db::orm::{AtomicTransaction, DatabaseConnection, Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use std::collections::BTreeSet;
use uuid::Uuid;

/// A sign-in retains its Display Attributes while its session is being started.
const RECENT_SIGN_IN: Duration = Duration::minutes(10);

pub(crate) async fn database_time<E: TransactionExecutor + ?Sized>(
	tx: &mut E,
) -> Result<DateTime<Utc>> {
	let (sql, values) = Query::select()
		.expr_as(Expr::cust("clock_timestamp()"), Alias::new("now"))
		.build(PostgresQueryBuilder);
	let row = TransactionExecutor::fetch_one(tx, &sql, convert_values(values)).await?;
	Ok(row.get("now").map_err(FrameworkError::from)?)
}

impl DashboardRegistrationRequest {
	pub(crate) fn contract(self) -> Registration {
		Registration {
			id: self.id,
			identity_id: self.identity_id(),
			status: self.status,
			created_at: self.created_at,
			expires_at: self.expires_at,
			decided_at: self.decided_at,
			tenant: self.tenant,
		}
	}

	/// The identity's latest request not made with an Invitation.
	pub(crate) async fn latest<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
	) -> Result<Option<Registration>> {
		Ok(Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter(Self::field_invitation_id().is_null())
			.order_by(&["-created_at", "-id"])
			.limit(1)
			.all_with_db(db)
			.await?
			.pop()
			.map(Self::contract))
	}

	/// The identity's latest request of any kind, including those made with an
	/// Invitation, for Display Attribute retention.
	async fn latest_of_any<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
	) -> Result<Option<Registration>> {
		Ok(Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.order_by(&["-created_at", "-id"])
			.limit(1)
			.all_with_db(db)
			.await?
			.pop()
			.map(Self::contract))
	}

	/// Whether the identity has any unexpired pending request.
	async fn pending_of_any<E: OrmExecutor>(
		db: &mut E,
		identity: Uuid,
		now: DateTime<Utc>,
	) -> Result<bool> {
		Ok(Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter(Self::field_status().eq("pending"))
			.filter(Self::field_expires_at().gt(now))
			.exists_with_db(db)
			.await?)
	}

	/// The External Identity a Registration Request belongs to; it never changes.
	pub(crate) async fn identity_of<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Option<Uuid>> {
		Ok(Self::objects()
			.filter(Self::field_id().eq(id))
			.all_with_db(db)
			.await?
			.pop()
			.map(|registration| registration.identity_id()))
	}

	/// Opens a Registration Request addressed to `tenant` through an
	/// Invitation. An identity has at most one pending request per Tenant, so a
	/// second one fails without spending the Invitation.
	pub(crate) async fn open_addressed(
		tx: &mut AtomicTransaction,
		identity: Uuid,
		tenant: &str,
		invitation: Option<Uuid>,
	) -> Result<Self> {
		// The identity lock serializes requests, as `submit` does. A
		// disablement that committed since the holder was checked is seen
		// here, before the Invitation is spent.
		let account = DashboardIdentity::objects()
			.filter(DashboardIdentity::field_id().eq(identity))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or(Error::Unauthorized)?;
		if account.disabled_at.is_some() {
			return Err(Error::Forbidden);
		}
		let now = database_time(tx).await?;
		if Self::objects()
			.filter(Self::field_identity_id().eq(identity))
			.filter(Self::field_tenant().eq(Some(tenant.to_owned())))
			.filter(Self::field_status().eq("pending"))
			.filter(Self::field_expires_at().gt(now))
			.exists_with_db(tx)
			.await?
		{
			return Err(Error::Conflict(
				"a request to this Tenant is already pending".into(),
			));
		}
		let registration = Self::build()
			.id(Uuid::new_v4())
			.identity_id(identity)
			.status("pending".to_owned())
			.created_at(now)
			.expires_at(now + Duration::days(7))
			.decided_at(None)
			.decided_by(None)
			.decision_actor(None)
			.tenant(Some(tenant.to_owned()))
			.invitation_id(invitation)
			.finish();
		Ok(Self::objects().create_with_conn(tx, &registration).await?)
	}

	/// Opens or returns the identity's own request, made without an
	/// Invitation. A Tenant Binding addresses it to the bound Tenant; otherwise
	/// it is addressed to no Tenant and only Deployment Administrators decide it.
	pub(crate) async fn submit(
		db: DatabaseConnection,
		identity: Uuid,
		tenant: Option<String>,
	) -> Result<Registration> {
		db.atomic(async |tx| {
			// One identity lock serializes duplicate submissions without extending
			// the original seven-day deadline.
			let identities = DashboardIdentity::objects()
				.filter(DashboardIdentity::field_id().eq(identity))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?;
			if identities.is_empty() {
				return Err(Error::Unauthorized);
			}
			let now = database_time(tx).await?;
			Self::objects()
				.filter(Self::field_identity_id().eq(identity))
				.filter(Self::field_status().eq("pending"))
				.filter(Self::field_expires_at().lte(now))
				.update_fields_with_conn(tx, [(Self::field_status(), "expired".to_owned())])
				.await?;
			// A replacement request retains the latest authenticated display
			// attributes. Expiry without resubmission is erased by the sweeper.
			let previous = Self::latest(tx, identity).await?;
			if let Some(previous) = &previous
				&& previous.status == "pending"
				&& previous.tenant != tenant
			{
				// A pending request addressed elsewhere, as when the identity's
				// GCIP pool was bound to another Tenant since, can no longer be
				// approved: it ends, and a request to the current address opens.
				Self::objects()
					.filter(Self::field_id().eq(previous.id))
					.update_fields_with_conn(tx, [(Self::field_status(), "expired".to_owned())])
					.await?;
			} else if let Some(previous) = previous {
				// A Personal Tenant's owner keeps that authority while requesting
				// additional shared membership. Each approval remains immutable and
				// a new request still requires an explicit approval.
				let personal = PersonalTenantLink::objects()
					.filter(PersonalTenantLink::field_identity_id().eq(identity))
					.exists_with_db(tx)
					.await?;
				let active_mapping = !personal
					&& previous.status == "approved"
					&& DashboardMapping::objects()
						.filter(DashboardMapping::field_identity_id().eq(identity))
						.filter(DashboardMapping::field_enabled().eq(true))
						.exists_with_db(tx)
						.await?;
				if dashboard_rules::reuse_registration(&previous, now, active_mapping)? {
					return Ok(previous);
				}
			}
			let registration = Self::build()
				.id(Uuid::new_v4())
				.identity_id(identity)
				.status("pending".to_owned())
				.created_at(now)
				.expires_at(now + Duration::days(7))
				.decided_at(None)
				.decided_by(None)
				.decision_actor(None)
				.tenant(tenant.clone())
				.invitation_id(None)
				.finish();
			Ok(Self::objects()
				.create_with_conn(tx, &registration)
				.await?
				.contract())
		})
		.await
	}

	pub(crate) async fn pending(db: DatabaseConnection) -> Result<Vec<Registration>> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			Ok(Self::objects()
				.filter(Self::field_status().eq("pending"))
				.filter(Self::field_expires_at().gt(now))
				.order_by(&["-created_at", "-id"])
				.all_with_db(tx)
				.await?
				.into_iter()
				.map(Self::contract)
				.collect())
		})
		.await
	}

	/// Approves into `input.tenant` and `input.subject`. A Tenant Administrator
	/// approves only into its own Tenant, and only a Registration Request from
	/// the bound GCIP Tenant; it creates the subject with `groups` or re-approves
	/// the subject that External Identity was previously mapped to.
	pub(crate) async fn approve(
		db: DatabaseConnection,
		id: Uuid,
		input: Approval,
		groups: BTreeSet<String>,
		actor: &AdministrationActor,
		token_hash: Vec<u8>,
		policy: Option<aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Result<ApprovedMapping> {
		db.atomic(async |tx| {
			let administrator = actor.tenant_administrator();
			// Authority, including its exclusive policy lock, precedes every
			// identity lock, as at execution boundaries.
			let mut snapshot = match administrator {
				Some(administrator) => Some(administrator.lock(tx, true).await?),
				None => None,
			};
			let preliminary = Self::objects()
				.filter(Self::field_id().eq(id))
				.all_with_db(tx)
				.await?
				.pop()
				.ok_or_else(|| Error::NotFound("registration".into()))?;
			let candidate = DashboardIdentity::find(tx, preliminary.identity_id()).await?;
			if let (Some(administrator), Some(snapshot)) = (administrator, &snapshot) {
				visible_to(administrator, &preliminary, &candidate)?;
				administrator
					.require(
						tx,
						snapshot,
						tenant_administration::REGISTRATION_APPROVE,
						&id.to_string(),
						json!({
							"identity_id": candidate.id,
							"subject": input.subject,
							"groups": groups,
						}),
					)
					.await?;
			}
			crate::authorization::identity::require_dashboard_mapping(
				policy.as_ref(),
				&candidate.issuer,
				&candidate.gcip_tenant,
				&input.tenant,
			)?;
			let mut snapshot = match snapshot.take() {
				Some(snapshot) => snapshot,
				None => {
					// QuerySet currently exposes FOR UPDATE only. This shared policy lock
					// allows other admissions while excluding policy replacement.
					let (sql, values) = Query::select()
						.column(Alias::new("revision"))
						.expr_as(
							Expr::col(Alias::new("document")).cast_as("text"),
							Alias::new("document"),
						)
						.from(Alias::new(AuthorizationBundle::table_name()))
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(reinhardt::query::Expr::value(input.tenant.clone())),
						)
						.lock(if input.groups.is_some() {
							LockType::Update
						} else {
							LockType::Share
						})
						.build(PostgresQueryBuilder);
					let row = TransactionExecutor::fetch_optional(tx, &sql, convert_values(values))
						.await?
						.ok_or_else(|| Error::NotFound("authorization policy".into()))?;
					Snapshot {
						revision: row.get("revision").map_err(FrameworkError::from)?,
						bundle: serde_json::from_str(
							&row.get::<String>("document")
								.map_err(FrameworkError::from)?,
						)?,
					}
				}
			};
			let identity = DashboardIdentity::objects()
				.filter(DashboardIdentity::field_id().eq(preliminary.identity_id()))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or(Error::Forbidden)?;
			let mut registration = Self::objects()
				.filter(Self::field_id().eq(id))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("registration".into()))?;
			let now = database_time(tx).await?;
			dashboard_rules::require_pending(&registration.status, registration.expires_at, now)?;
			if identity.disabled_at.is_some() {
				return Err(Error::Forbidden);
			}
			if registration
				.tenant
				.as_ref()
				.is_some_and(|addressed| addressed != &input.tenant)
			{
				return Err(Error::Invalid(
					"the request is addressed to another Tenant".into(),
				));
			}
			if administrator.is_none()
				&& !snapshot.bundle.subjects.contains_key(&input.subject)
				&& let Some(groups) = &input.groups
			{
				// A Deployment Administrator may already replace the whole
				// policy, so its new subject is not limited to Assignable Groups.
				if let Some(group) = groups
					.iter()
					.find(|group| !snapshot.bundle.groups.contains_key(*group))
				{
					return Err(Error::Invalid(format!("unknown group: {group}")));
				}
				aidash_domain::policy::identifier(&input.subject)?;
				snapshot.bundle.subjects.insert(
					input.subject.clone(),
					aidash_domain::policy::Subject {
						kind: aidash_domain::policy::SubjectKind::User,
						groups: groups.clone(),
						roles: Default::default(),
						attributes: json!({}),
						enabled: true,
						delegated_by: None,
					},
				);
				snapshot.bundle.validate()?;
				snapshot.revision = AuthorizationBundle::replace(
					tx,
					&input.tenant,
					snapshot.revision,
					serde_json::to_value(&snapshot.bundle)?,
					"deployment_administrator",
				)
				.await?;
			}
			crate::authorization::identity::require_dashboard_mapping(
				policy.as_ref(),
				&identity.issuer,
				&identity.gcip_tenant,
				&input.tenant,
			)?;
			if let Some(administrator) = administrator {
				if !snapshot.bundle.subjects.contains_key(&input.subject)
					&& DashboardMapping::objects()
						.filter(DashboardMapping::field_tenant().eq(input.tenant.clone()))
						.filter(DashboardMapping::field_subject().eq(input.subject.clone()))
						.exists_with_db(tx)
						.await?
				{
					return Err(Error::Conflict("subject was previously assigned".into()));
				}
				let previously_mapped = DashboardMapping::objects()
					.filter(DashboardMapping::field_identity_id().eq(identity.id))
					.filter(DashboardMapping::field_tenant().eq(input.tenant.clone()))
					.filter(DashboardMapping::field_subject().eq(input.subject.clone()))
					.exists_with_db(tx)
					.await?;
				if tenant_administration::approval_target(
					&snapshot.bundle,
					&input.subject,
					previously_mapped,
					&groups,
				)? == tenant_administration::ApprovalTarget::NewSubject
				{
					tenant_administration::add_user_subject(
						&mut snapshot.bundle,
						&input.subject,
						&groups,
					)?;
					snapshot.revision = administrator.save_policy(tx, &snapshot).await?;
				}
			}
			dashboard_rules::require_user_mapping(&snapshot, &input.subject)?;
			// The secret never leaves the service; this record anchors a policy
			// lease and cannot be used as a bearer credential.
			let credential = AuthorizationCredential::build()
				.id(Uuid::new_v4())
				.tenant(input.tenant.clone())
				.subject(input.subject.clone())
				.token_hash(token_hash)
				.created_at(now)
				.expires_at(now + Duration::days(3650))
				.revoked_at(None)
				.issued_by("dashboard-oidc".to_owned())
				.finish();
			AuthorizationCredential::objects()
				.create_with_conn(tx, &credential)
				.await?;
			let identity_id = registration.identity_id();
			let existing = DashboardMapping::objects()
				.filter(DashboardMapping::field_identity_id().eq(identity_id))
				.filter(DashboardMapping::field_tenant().eq(input.tenant.clone()))
				.filter(DashboardMapping::field_subject().eq(input.subject.clone()))
				.all_with_db(tx)
				.await?
				.pop();
			let mapping_id = if let Some(mapping) = existing {
				if mapping.enabled {
					return Err(Error::Conflict("identity already has this mapping".into()));
				}
				let assignments: Vec<FieldAssignment> = vec![
					(DashboardMapping::field_credential_id(), credential.id).into(),
					(DashboardMapping::field_enabled(), true).into(),
					(
						DashboardMapping::field_revision(),
						dashboard_rules::next_revision(mapping.revision)?,
					)
						.into(),
				];
				DashboardMapping::objects()
					.filter(DashboardMapping::field_id().eq(mapping.id))
					.update_fields_with_conn(tx, assignments)
					.await?;
				mapping.id
			} else {
				// Preserve a conflict result for concurrent approvals for the same
				// identity, including approvals of different registration records.
				let id = Uuid::new_v4();
				let (sql, values) = Query::insert()
					.into_table(Alias::new(DashboardMapping::table_name()))
					.columns(
						["id", "identity_id", "tenant", "subject", "credential_id"].map(Alias::new),
					)
					.values_panic([
						IntoValue::into_value(id),
						IntoValue::into_value(identity_id),
						IntoValue::into_value(input.tenant.clone()),
						IntoValue::into_value(input.subject.clone()),
						IntoValue::into_value(credential.id),
					])
					.on_conflict(
						OnConflict::columns(["identity_id", "tenant", "subject"])
							.do_nothing()
							.to_owned(),
					)
					.build(PostgresQueryBuilder);
				if TransactionExecutor::execute(tx, &sql, convert_values(values))
					.await?
					.rows_affected != 1
				{
					return Err(Error::Conflict("identity already has this mapping".into()));
				}
				id
			};
			registration.status = "approved".into();
			registration.decided_at = Some(now);
			registration.decision_actor = Some(actor.decision_actor());
			Self::objects().update_with_conn(tx, &registration).await?;
			actor
				.record(
					tx,
					AdministrationEvent {
						action: tenant_administration::REGISTRATION_APPROVE,
						identity: Some(identity_id),
						registration: Some(id),
						mapping: Some(mapping_id),
						tenant: Some(&input.tenant),
						subject: Some(&input.subject),
						// Correlates the decision with the Invitation it came from.
						invitation: registration.invitation_id,
					},
				)
				.await?;
			Ok(ApprovedMapping {
				id: mapping_id,
				identity_id,
				tenant: input.tenant,
				subject: input.subject,
			})
		})
		.await
	}

	pub(crate) async fn reject(
		db: DatabaseConnection,
		id: Uuid,
		actor: &AdministrationActor,
		idle_seconds: Option<i64>,
	) -> Result<Registration> {
		db.atomic(async |tx| {
			let administrator = actor.tenant_administrator();
			let snapshot = match administrator {
				Some(administrator) => Some(administrator.lock(tx, false).await?),
				None => None,
			};
			let preliminary = Self::objects()
				.filter(Self::field_id().eq(id))
				.all_with_db(tx)
				.await?
				.pop()
				.ok_or_else(|| Error::Conflict("registration is no longer pending".into()))?;
			if let (Some(administrator), Some(snapshot)) = (administrator, &snapshot) {
				let candidate = DashboardIdentity::find(tx, preliminary.identity_id()).await?;
				visible_to(administrator, &preliminary, &candidate)?;
				administrator
					.require(
						tx,
						snapshot,
						tenant_administration::REGISTRATION_REJECT,
						&id.to_string(),
						json!({"identity_id": candidate.id}),
					)
					.await?;
			}
			DashboardIdentity::objects()
				.filter(DashboardIdentity::field_id().eq(preliminary.identity_id()))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?;
			let mut row = Self::objects()
				.filter(Self::field_id().eq(id))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::Conflict("registration is no longer pending".into()))?;
			let now = database_time(tx).await?;
			dashboard_rules::require_pending(&row.status, row.expires_at, now)?;
			row.status = "rejected".into();
			row.decided_at = Some(now);
			row.decision_actor = Some(actor.decision_actor());
			Self::objects().update_with_conn(tx, &row).await?;
			DashboardIdentity::clear_display_without_basis(tx, row.identity_id(), idle_seconds)
				.await?;
			actor
				.record(
					tx,
					AdministrationEvent {
						action: tenant_administration::REGISTRATION_REJECT,
						identity: Some(row.identity_id()),
						registration: Some(id),
						mapping: None,
						// A Deployment Administrator's rejection still names the
						// Tenant an addressed request was for.
						tenant: administrator
							.map(TenantAdministrator::tenant)
							.or(row.tenant.as_deref()),
						subject: None,
						invitation: row.invitation_id,
					},
				)
				.await?;
			Ok(row.contract())
		})
		.await
	}
}

/// A Tenant Administrator sees only Registration Requests from the configured
/// issuer's GCIP Tenant bound to its Tenant, and never acts on its own External
/// Identity.
pub(super) fn visible_to(
	administrator: &TenantAdministrator,
	registration: &DashboardRegistrationRequest,
	identity: &DashboardIdentity,
) -> Result<()> {
	if !administrator.sees(registration, identity) || identity.id == administrator.identity_id {
		return Err(Error::NotFound("registration".into()));
	}
	Ok(())
}

impl DashboardIdentity {
	/// Clears Display Attributes once the External Identity has no retention basis.
	///
	/// Authority (an enabled Mapping or Deployment Administrator grant) or an unexpired pending
	/// Registration Request retains them. A live session or a sign-in within
	/// [`RECENT_SIGN_IN`] retains them only when it began after the latest
	/// Registration Request ended, so rejection and expiry still clear the
	/// attributes the request showed, while a fresh sign-in keeps its newly
	/// verified claims. `idle_seconds` is the browser session idle limit, or
	/// `None` when dashboard sign-in is not configured: then no session is usable
	/// or still starting, so neither sessions nor recent sign-ins retain them.
	/// Callers hold no other identity lock, or only this one.
	pub(crate) async fn clear_display_without_basis<E: OrmExecutor + TransactionExecutor>(
		db: &mut E,
		identity: Uuid,
		idle_seconds: Option<i64>,
	) -> Result<()> {
		let Some(row) = Self::objects()
			.filter(Self::field_id().eq(identity))
			.select_for_update()
			.all_with_executor(db)
			.await
			.map_err(FrameworkError::from)?
			.pop()
		else {
			return Ok(());
		};
		if row.verified_email.is_none() && row.display_name.is_none() {
			return Ok(());
		}
		if row.disabled_at.is_none() && Self::retains_display(db, &row, idle_seconds).await? {
			return Ok(());
		}
		let assignments: Vec<FieldAssignment> = vec![
			(Self::field_verified_email(), None::<String>).into(),
			(Self::field_display_name(), None::<String>).into(),
		];
		Self::objects()
			.filter(Self::field_id().eq(identity))
			.update_fields_with_conn(db, assignments)
			.await?;
		Ok(())
	}

	async fn retains_display<E: OrmExecutor + TransactionExecutor>(
		db: &mut E,
		row: &Self,
		idle_seconds: Option<i64>,
	) -> Result<bool> {
		if DashboardMapping::objects()
			.filter(DashboardMapping::field_identity_id().eq(row.id))
			.filter(DashboardMapping::field_enabled().eq(true))
			.exists_with_db(db)
			.await? || DashboardDeploymentAdministratorGrant::objects()
			.filter(DashboardDeploymentAdministratorGrant::field_identity_id().eq(row.id))
			.filter(DashboardDeploymentAdministratorGrant::field_enabled().eq(true))
			.exists_with_db(db)
			.await?
		{
			return Ok(true);
		}
		let now = database_time(db).await?;
		// Every pending request retains them, including one made with an
		// Invitation, which its Tenant's administrators recognize the person by.
		if DashboardRegistrationRequest::pending_of_any(db, row.id, now).await? {
			return Ok(true);
		}
		let latest = DashboardRegistrationRequest::latest_of_any(db, row.id).await?;
		// A request ends at its decision, or at its deadline when it expired.
		let ended = latest.map(|request| match request.status.as_str() {
			"pending" | "expired" => request.expires_at,
			_ => request.decided_at.unwrap_or(request.created_at),
		});
		let after_request = |at: DateTime<Utc>| ended.is_none_or(|ended| at >= ended);
		// Without dashboard sign-in no session is usable, and none can still be starting.
		let Some(idle_seconds) = idle_seconds else {
			return Ok(false);
		};
		if row
			.display_observed_at
			.is_some_and(|observed| observed > now - RECENT_SIGN_IN && after_request(observed))
		{
			return Ok(true);
		}
		// Evaluate the complete retention predicate in PostgreSQL. An identity can
		// have arbitrarily many expired or idle sessions, so loading them all here
		// would let a sign-in consume unbounded memory before finding a live one.
		let mut sessions = Query::select();
		sessions
			.expr(Expr::value(1_i32))
			.from(Alias::new(DashboardSession::table_name()))
			.and_where(Expr::col(Alias::new("identity_id")).eq(Expr::value(row.id)))
			.and_where(Expr::col(Alias::new("revoked_at")).is_null())
			.and_where(Expr::col(Alias::new("expires_at")).gt(Expr::value(now)))
			.and_where(Expr::col(Alias::new("last_activity_at")).gt(
				SimpleExpr::CustomWithExpr(
					"?::timestamptz - make_interval(secs => coalesce(desktop_idle_seconds::double precision, ?))".into(),
					vec![Expr::value(now).into(), Expr::value(idle_seconds as f64).into()],
				),
			))
			.limit(1);
		if let Some(ended) = ended {
			sessions.and_where(Expr::col(Alias::new("created_at")).gte(Expr::value(ended)));
		}
		let (sql, values) = sessions.build(PostgresQueryBuilder);
		Ok(
			TransactionExecutor::fetch_optional(db, &sql, convert_values(values))
				.await?
				.is_some(),
		)
	}

	/// Expires overdue Registration Requests and enforces display retention.
	///
	/// Each External Identity is handled in its own transaction, locked in id order,
	/// and rechecked under that lock, so a concurrent sign-in either commits first
	/// and is seen, or waits and overwrites the cleared attributes afterwards.
	/// Enabled authority is rechecked there too; skipping it here only avoids
	/// locking every mapped External Identity on each pass. `idle_seconds` follows
	/// [`Self::clear_display_without_basis`].
	pub(crate) async fn enforce_display_retention(
		db: DatabaseConnection,
		idle_seconds: Option<i64>,
	) -> Result<()> {
		let candidates = db
			.atomic(async |tx| {
				let now = database_time(tx).await?;
				let mut authorized: BTreeSet<Uuid> = DashboardMapping::objects()
					.filter(DashboardMapping::field_enabled().eq(true))
					.all_with_db(tx)
					.await?
					.into_iter()
					.map(|row| row.identity_id)
					.collect();
				authorized.extend(
					DashboardDeploymentAdministratorGrant::objects()
						.filter(DashboardDeploymentAdministratorGrant::field_enabled().eq(true))
						.all_with_db(tx)
						.await?
						.into_iter()
						.map(|row| row.identity_id()),
				);
				let mut ids = BTreeSet::new();
				for shown in [
					Self::field_verified_email().is_not_null(),
					Self::field_display_name().is_not_null(),
				] {
					ids.extend(
						Self::objects()
							.filter(shown)
							.all_with_db(tx)
							.await?
							.into_iter()
							.filter(|row| {
								row.disabled_at.is_some() || !authorized.contains(&row.id)
							})
							.map(|row| row.id),
					);
				}
				ids.extend(
					DashboardRegistrationRequest::objects()
						.filter(DashboardRegistrationRequest::field_status().eq("pending"))
						.filter(DashboardRegistrationRequest::field_expires_at().lte(now))
						.all_with_db(tx)
						.await?
						.into_iter()
						.map(|row| row.identity_id()),
				);
				Ok::<_, Error>(ids)
			})
			.await?;
		for identity in candidates {
			Self::enforce_identity_retention(db, identity, idle_seconds).await?;
		}
		Ok(())
	}

	/// Applies [`Self::enforce_display_retention`] to one External Identity.
	pub(crate) async fn enforce_identity_retention(
		db: DatabaseConnection,
		identity: Uuid,
		idle_seconds: Option<i64>,
	) -> Result<()> {
		db.atomic(async |tx| {
			Self::objects()
				.filter(Self::field_id().eq(identity))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?;
			let now = database_time(tx).await?;
			DashboardRegistrationRequest::objects()
				.filter(DashboardRegistrationRequest::field_identity_id().eq(identity))
				.filter(DashboardRegistrationRequest::field_status().eq("pending"))
				.filter(DashboardRegistrationRequest::field_expires_at().lte(now))
				.update_fields_with_conn(
					tx,
					[(
						DashboardRegistrationRequest::field_status(),
						"expired".to_owned(),
					)],
				)
				.await?;
			Self::clear_display_without_basis(tx, identity, idle_seconds).await
		})
		.await
	}

	/// Performs a Deployment Administrator's Display Erasure, keeping the first erasure's trace.
	pub(crate) async fn erase_display(
		db: DatabaseConnection,
		id: Uuid,
		actor: String,
	) -> Result<IdentityView> {
		db.atomic(async |tx| {
			let mut row = Self::objects()
				.filter(Self::field_id().eq(id))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("identity".into()))?;
			if row.display_erased_at.is_none() {
				row.display_erased_at = Some(database_time(tx).await?);
				row.display_erased_by = Some(actor);
			}
			row.verified_email = None;
			row.display_name = None;
			Self::objects().update_with_conn(tx, &row).await?;
			Ok(row.contract())
		})
		.await
	}
	pub(crate) fn contract(self) -> IdentityView {
		IdentityView {
			id: self.id,
			issuer: self.issuer,
			subject: self.subject,
			gcip_tenant: (!self.gcip_tenant.is_empty()).then_some(self.gcip_tenant),
			verified_email: self.verified_email,
			display_name: self.display_name,
			disabled_at: self.disabled_at,
			display_erased_at: self.display_erased_at,
			display_erased_by: self.display_erased_by,
		}
	}

	pub(crate) async fn find<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Self> {
		Self::objects()
			.filter(Self::field_id().eq(id))
			.all_with_db(db)
			.await?
			.pop()
			.ok_or_else(|| Error::NotFound("identity".into()))
	}

	pub(crate) async fn page<E: OrmExecutor>(db: &mut E, offset: u64) -> Result<Vec<IdentityView>> {
		Ok(Self::objects()
			.all()
			.order_by(&["subject", "id"])
			.limit(200)
			.offset(offset as usize)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(Self::contract)
			.collect())
	}

	pub(crate) async fn restore<E: OrmExecutor>(
		db: &mut E,
		id: Uuid,
		checked_at: DateTime<Utc>,
	) -> Result<()> {
		let checked_at = crate::database::postgres_timestamp(checked_at);
		let assignments: Vec<FieldAssignment> = vec![
			(Self::field_disabled_at(), None::<DateTime<Utc>>).into(),
			(Self::field_last_valid_at(), Some(checked_at)).into(),
		];
		let changed = Self::objects()
			.filter(Self::field_id().eq(id))
			.filter(Self::field_disabled_at().is_not_null())
			.update_fields_with_conn(db, assignments)
			.await?;
		if changed != 1 {
			return Err(Error::Conflict("identity status changed".into()));
		}
		Ok(())
	}
}

#[cfg(test)]
#[path = "../tests/database_timestamps.rs"]
mod timestamp_tests;

impl DashboardMapping {
	pub(crate) async fn find<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Option<Self>> {
		Ok(Self::objects()
			.filter(Self::field_id().eq(id))
			.all_with_db(db)
			.await?
			.pop())
	}

	pub(crate) async fn page<E: OrmExecutor>(db: &mut E, offset: u64) -> Result<Vec<AdminMapping>> {
		Ok(Self::objects()
			.all()
			.order_by(&["tenant", "subject", "id"])
			.limit(200)
			.offset(offset as usize)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| AdminMapping {
				id: row.id,
				identity_id: row.identity_id(),
				tenant: row.tenant,
				subject: row.subject,
				enabled: row.enabled,
				revision: row.revision,
			})
			.collect())
	}

	/// A Tenant Administrator disables only another External Identity's Mapping
	/// in its own Tenant.
	pub(crate) async fn disable(
		db: DatabaseConnection,
		id: Uuid,
		revision: i64,
		actor: &AdministrationActor,
	) -> Result<()> {
		db.atomic(async |tx| {
			if let Some(administrator) = actor.tenant_administrator() {
				// Each administrator holds a shared lock on its own credential, so
				// two administrators disabling each other's Mappings would deadlock
				// on the target credential below. The exclusive policy lock
				// serializes them before any credential lock is taken.
				let snapshot = administrator.lock(tx, true).await?;
				let target = Self::find(tx, id)
					.await?
					.filter(|mapping| mapping.tenant == administrator.tenant())
					.ok_or_else(|| Error::NotFound("mapping".into()))?;
				if target.identity_id() == administrator.identity_id {
					return Err(Error::Forbidden);
				}
				administrator
					.require(
						tx,
						&snapshot,
						tenant_administration::MAPPING_DISABLE,
						&id.to_string(),
						json!({"identity_id": target.identity_id(), "subject": target.subject}),
					)
					.await?;
				administrator
					.require_remaining_administrator(tx, &snapshot.bundle, Some(id))
					.await?;
			}
			let mapping = Self::objects()
				.filter(Self::field_id().eq(id))
				.filter(Self::field_enabled().eq(true))
				.filter(Self::field_revision().eq(revision))
				.all_with_db(tx)
				.await?
				.pop()
				.ok_or_else(|| Error::Conflict("mapping revision changed".into()))?;
			// Execution leases always lock the credential before the mapping.
			let mut credential = AuthorizationCredential::objects()
				.filter(AuthorizationCredential::field_id().eq(mapping.credential_id()))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("credential".into()))?;
			let assignments: Vec<FieldAssignment> = vec![
				(Self::field_enabled(), false).into(),
				(
					Self::field_revision(),
					dashboard_rules::next_revision(revision)?,
				)
					.into(),
			];
			let changed = Self::objects()
				.filter(Self::field_id().eq(id))
				.filter(Self::field_enabled().eq(true))
				.filter(Self::field_revision().eq(revision))
				.filter(Self::field_credential_id().eq(credential.id))
				.update_fields_with_conn(tx, assignments)
				.await?;
			if changed != 1 {
				return Err(Error::Conflict("mapping revision changed".into()));
			}
			if credential.revoked_at.is_none() {
				credential.revoked_at = Some(database_time(tx).await?);
				AuthorizationCredential::objects()
					.update_with_conn(tx, &credential)
					.await?;
			}
			actor
				.record(
					tx,
					AdministrationEvent {
						action: tenant_administration::MAPPING_DISABLE,
						identity: Some(mapping.identity_id()),
						registration: None,
						mapping: Some(id),
						tenant: Some(&mapping.tenant),
						subject: Some(&mapping.subject),
						invitation: None,
					},
				)
				.await?;
			Ok(())
		})
		.await
	}
}

impl DashboardDeploymentAdministratorGrant {
	pub(crate) fn contract(self) -> DeploymentAdministratorGrant {
		DeploymentAdministratorGrant {
			identity_id: self.identity_id(),
			enabled: self.enabled,
			revision: self.revision,
		}
	}

	pub(crate) async fn list<E: OrmExecutor>(
		db: &mut E,
	) -> Result<Vec<DeploymentAdministratorGrant>> {
		Ok(Self::objects()
			.all()
			.order_by(&["identity_id"])
			.all_with_db(db)
			.await?
			.into_iter()
			.map(Self::contract)
			.collect())
	}

	pub(crate) async fn change_enabled(
		db: DatabaseConnection,
		identity: Uuid,
		enabled: bool,
		revision: i64,
	) -> Result<DeploymentAdministratorGrant> {
		db.atomic(async |tx| {
			let identity = DashboardIdentity::objects()
				.filter(DashboardIdentity::field_id().eq(identity))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("identity".into()))?;
			if enabled && identity.disabled_at.is_some() {
				return Err(Error::Forbidden);
			}
			let existing = Self::objects()
				.filter(Self::field_identity_id().eq(identity.id))
				.all_with_db(tx)
				.await?
				.pop();
			let grant = if let Some(mut grant) = existing {
				if grant.revision != revision {
					return Err(Error::Conflict(
						"Deployment Administrator grant revision changed".into(),
					));
				}
				grant.enabled = enabled;
				grant.revision = dashboard_rules::next_revision(grant.revision)?;
				Self::objects().update_with_conn(tx, &grant).await?
			} else {
				if revision != 0 {
					return Err(Error::Conflict(
						"Deployment Administrator grant revision changed".into(),
					));
				}
				let grant = Self::build()
					.identity_id(identity.id)
					.enabled(enabled)
					.revision(1)
					.finish();
				Self::objects().create_with_conn(tx, &grant).await?
			};
			Ok(grant.contract())
		})
		.await
	}
}

use reinhardt::query::IntoValue;
