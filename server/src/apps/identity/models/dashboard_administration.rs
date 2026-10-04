//! Transactional dashboard registration and authority changes.

use super::{
	AuthorizationBundle, AuthorizationCredential, DashboardIdentity, DashboardMapping,
	DashboardOperatorGrant, DashboardRegistrationRequest,
};
use crate::apps::identity::serializers::oidc::{
	AdminMapping, AdminOperatorGrant, Approval, ApprovedMapping, IdentityView, Registration,
};
use crate::apps::identity::services::dashboard_rules;
use crate::authorization::Snapshot;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::query::FieldAssignment;
use reinhardt::db::orm::{AtomicTransaction, DatabaseConnection, Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

pub(super) async fn database_time(tx: &mut AtomicTransaction) -> Result<DateTime<Utc>> {
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
		}
	}

	pub(crate) async fn latest<E: OrmExecutor>(
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

	pub(crate) async fn submit(db: DatabaseConnection, identity: Uuid) -> Result<Registration> {
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
			if let Some(previous) = Self::latest(tx, identity).await? {
				let active_mapping = previous.status == "approved"
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

	pub(crate) async fn approve(
		db: DatabaseConnection,
		id: Uuid,
		input: Approval,
		decision_actor: String,
		token_hash: Vec<u8>,
	) -> Result<ApprovedMapping> {
		db.atomic(async |tx| {
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
				.lock(LockType::Share)
				.build(PostgresQueryBuilder);
			let row = TransactionExecutor::fetch_optional(tx, &sql, convert_values(values))
				.await?
				.ok_or_else(|| Error::NotFound("authorization policy".into()))?;
			let snapshot = Snapshot {
				revision: row.get("revision").map_err(FrameworkError::from)?,
				bundle: serde_json::from_str(
					&row.get::<String>("document")
						.map_err(FrameworkError::from)?,
				)?,
			};
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
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
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
			registration.decision_actor = Some(decision_actor);
			Self::objects().update_with_conn(tx, &registration).await?;
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
		actor: String,
	) -> Result<Registration> {
		db.atomic(async |tx| {
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
			row.decision_actor = Some(actor);
			Self::objects().update_with_conn(tx, &row).await?;
			Ok(row.contract())
		})
		.await
	}
}

impl DashboardIdentity {
	pub(crate) fn contract(self) -> IdentityView {
		IdentityView {
			id: self.id,
			issuer: self.issuer,
			subject: self.subject,
			disabled_at: self.disabled_at,
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

	pub(crate) async fn disable(db: DatabaseConnection, id: Uuid, revision: i64) -> Result<()> {
		db.atomic(async |tx| {
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
			Ok(())
		})
		.await
	}
}

impl DashboardOperatorGrant {
	pub(crate) fn contract(self) -> AdminOperatorGrant {
		AdminOperatorGrant {
			identity_id: self.identity_id(),
			enabled: self.enabled,
			revision: self.revision,
		}
	}

	pub(crate) async fn list<E: OrmExecutor>(db: &mut E) -> Result<Vec<AdminOperatorGrant>> {
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
	) -> Result<AdminOperatorGrant> {
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
					return Err(Error::Conflict("operator grant revision changed".into()));
				}
				grant.enabled = enabled;
				grant.revision = dashboard_rules::next_revision(grant.revision)?;
				Self::objects().update_with_conn(tx, &grant).await?
			} else {
				if revision != 0 {
					return Err(Error::Conflict("operator grant revision changed".into()));
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
