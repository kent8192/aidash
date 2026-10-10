//! Browser authentication state and provider-status persistence.

use super::dashboard_administration::database_time;
use super::{
	DashboardIdentity, DashboardLoginTransaction, DashboardLogoutToken,
	DashboardRegistrationRequest, DashboardSession,
};
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{DatabaseConnection, Model, OrmExecutor};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, IntoIden, JoinType, LockType, OnConflict,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr, TableRef,
};
use std::result::Result as StdResult;
use uuid::Uuid;

impl DashboardLoginTransaction {
	pub(crate) async fn consume_bound(
		db: DatabaseConnection,
		state: Vec<u8>,
		browser: Vec<u8>,
	) -> Result<Self> {
		db.atomic(async |tx| {
			let row = Self::objects()
				.filter(Self::field_state_hash().eq(super::byte_key::ByteKey(state)))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or(Error::Unauthorized)?;
			if row.browser_hash != browser
				|| row.expires_at <= database_time(tx).await?
				|| row.gcip_tenant.is_none()
			{
				return Err(Error::Unauthorized);
			}
			Self::objects()
				.filter(Self::field_state_hash().eq(row.state_hash.clone()))
				.delete_with_conn(tx)
				.await?;
			Ok(row)
		})
		.await
	}

	pub(crate) async fn reserve(&self, db: DatabaseConnection) -> Result<()> {
		db.atomic(async |tx| {
			// Keep capacity admission short; discovery happens after commit.
			let (sql, values) = Query::select()
				.expr(SimpleExpr::FunctionCall(
					Alias::new("pg_advisory_xact_lock").into_iden(),
					vec![Expr::value(71_003_204_i64).into()],
				))
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			let now = database_time(tx).await?;
			Self::objects()
				.filter(Self::field_expires_at().lte(now))
				.delete_with_conn(tx)
				.await?;
			let total = Self::objects().all().count_with_db(tx).await?;
			let pending = Self::objects()
				.filter(Self::field_browser_hash().eq(self.browser_hash.clone()))
				.count_with_db(tx)
				.await?;
			if total >= 10_000 || pending >= 8 {
				return Err(Error::Conflict("too many pending sign-ins".into()));
			}
			Self::objects().create_with_conn(tx, self).await?;
			Ok(())
		})
		.await
	}

	pub(crate) async fn release<E: OrmExecutor>(db: &mut E, state: Vec<u8>) -> Result<()> {
		Self::objects()
			.filter(Self::field_state_hash().eq(super::byte_key::ByteKey(state)))
			.delete_with_conn(db)
			.await?;
		Ok(())
	}

	pub(crate) async fn consume(db: DatabaseConnection, state: Vec<u8>) -> Result<Self> {
		db.atomic(async |tx| {
			let row = Self::objects()
				.filter(Self::field_state_hash().eq(super::byte_key::ByteKey(state)))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or(Error::Unauthorized)?;
			Self::objects()
				.filter(Self::field_state_hash().eq(row.state_hash.clone()))
				.delete_with_conn(tx)
				.await?;
			Ok(row)
		})
		.await
	}
}

impl DashboardIdentity {
	pub(crate) async fn register(
		db: DatabaseConnection,
		issuer: &str,
		sign_in: &aidash_domain::identity::dashboard::SignIn,
	) -> Result<Self> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			let subject = &sign_in.subject;
			let gcip_tenant = sign_in.gcip_tenant.as_deref().unwrap_or("");
			let (sql, values) = Query::insert()
				.into_table(Alias::new(Self::table_name()))
				.columns(
					[
						"id",
						"issuer",
						"subject",
						"last_valid_at",
						"gcip_tenant",
						"verified_email",
						"display_name",
						"display_observed_at",
					]
					.map(Alias::new),
				)
				.values_panic([
					IntoValue::into_value(Uuid::new_v4()),
					IntoValue::into_value(issuer),
					IntoValue::into_value(subject.clone()),
					IntoValue::into_value(now),
					IntoValue::into_value(gcip_tenant),
					IntoValue::into_value(sign_in.verified_email.clone()),
					IntoValue::into_value(sign_in.display_name.clone()),
					IntoValue::into_value(now),
				])
				.on_conflict(
					OnConflict::columns(["issuer", "gcip_tenant", "subject"])
						.update_columns(["display_observed_at"])
						.to_owned(),
				)
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			let mut identity = Self::objects()
				.filter(Self::field_issuer().eq(issuer))
				.filter(Self::field_subject().eq(subject.clone()))
				.filter(Self::field_gcip_tenant().eq(gcip_tenant))
				.all_with_db(tx)
				.await?
				.pop()
				.ok_or_else(|| Error::NotFound("identity".into()))?;
			// The upsert holds the identity lock. Display Erasure is permanent, so an
			// erased External Identity never records Display Attributes again.
			if identity.display_erased_at.is_none() {
				identity.verified_email = sign_in.verified_email.clone();
				identity.display_name = sign_in.display_name.clone();
				Self::objects().update_with_conn(tx, &identity).await?;
			}
			// End old requests before exposing refreshed claims. Display retention
			// keeps this sign-in's claims because it began after those requests.
			DashboardRegistrationRequest::objects()
				.filter(DashboardRegistrationRequest::field_identity_id().eq(identity.id))
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
			Ok(identity)
		})
		.await
	}

	pub(crate) async fn record_valid(
		db: DatabaseConnection,
		id: Uuid,
		checked_at: DateTime<Utc>,
		valid_since: Option<DateTime<Utc>>,
	) -> Result<()> {
		db.atomic(async |tx| {
			let mut row = Self::objects()
				.filter(Self::field_id().eq(id))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or(Error::Forbidden)?;
			if row.disabled_at.is_some() {
				return Err(Error::Forbidden);
			}
			row.last_valid_at = Some(
				row.last_valid_at
					.map_or(checked_at, |last| last.max(checked_at)),
			);
			row.valid_since = match (row.valid_since, valid_since) {
				(Some(previous), Some(current)) => Some(previous.max(current)),
				(previous, current) => previous.or(current),
			};
			Self::objects().update_with_conn(tx, &row).await?;
			if let Some(since) = row.valid_since {
				let revoked_at = database_time(tx).await?;
				DashboardSession::objects()
					.filter(DashboardSession::field_identity_id().eq(id))
					.filter(DashboardSession::field_revoked_at().is_null())
					.filter(DashboardSession::field_auth_time().lt(Some(since)))
					.update_fields_with_conn(
						tx,
						[(DashboardSession::field_revoked_at(), Some(revoked_at))],
					)
					.await?;
			}
			Ok(())
		})
		.await
	}

	pub(crate) async fn disable_if_current(
		db: DatabaseConnection,
		id: Uuid,
		checked_at: Option<DateTime<Utc>>,
	) -> Result<bool> {
		db.atomic(async |tx| {
			let row = Self::objects()
				.filter(Self::field_id().eq(id))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop();
			let Some(mut row) = row else {
				return Ok(false);
			};
			if row.disabled_at.is_some()
				|| checked_at
					.is_some_and(|checked| row.last_valid_at.is_some_and(|last| last > checked))
			{
				return Ok(false);
			}
			let now = database_time(tx).await?;
			row.disabled_at = Some(now);
			Self::objects().update_with_conn(tx, &row).await?;
			DashboardSession::objects()
				.filter(DashboardSession::field_identity_id().eq(id))
				.filter(DashboardSession::field_revoked_at().is_null())
				.update_fields_with_conn(tx, [(DashboardSession::field_revoked_at(), Some(now))])
				.await?;
			Ok(true)
		})
		.await
	}

	pub(crate) async fn active(db: DatabaseConnection, idle_seconds: i64) -> Result<Vec<Self>> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			DashboardSession::objects()
				.filter(DashboardSession::field_expires_at().lte(now))
				.delete_with_conn(tx)
				.await?;
			let sessions = Query::select()
				.column((Alias::new("s"), Alias::new("id")))
				.from_as(Alias::new(DashboardSession::table_name()), Alias::new("s"))
				.and_where(
					Expr::col((Alias::new("s"), Alias::new("identity_id")))
						.equals((Alias::new(Self::table_name()), Alias::new("id"))),
				)
				.and_where(Expr::col((Alias::new("s"), Alias::new("revoked_at"))).is_null())
				.and_where(
					Expr::col((Alias::new("s"), Alias::new("expires_at"))).gt(Expr::value(now)),
				)
				.and_where(
					Expr::col((Alias::new("s"), Alias::new("last_activity_at")))
						.gt(SimpleExpr::CustomWithExpr("clock_timestamp()-make_interval(secs => coalesce(s.desktop_idle_seconds::double precision,?))".into(), vec![Expr::value(idle_seconds as f64).into()])),
				)
				.to_owned();
			let runs = Query::select()
				.column((Alias::new("o"), Alias::new("run_id")))
				.from_as(Alias::new("dashboard_execution_origins"), Alias::new("o"))
				.join(
					JoinType::InnerJoin,
					TableRef::table_alias(Alias::new("runs"), Alias::new("r")),
					Expr::col((Alias::new("o"), Alias::new("run_id")))
						.equals((Alias::new("r"), Alias::new("id"))),
				)
				.and_where(
					Expr::col((Alias::new("o"), Alias::new("identity_id")))
						.equals((Alias::new(Self::table_name()), Alias::new("id"))),
				)
				.and_where(
					Expr::col((Alias::new("r"), Alias::new("phase"))).is_not_in([
						"COMPLETED",
						"FAILED",
						"CANCELLED",
					]),
				)
				.to_owned();
			let (sql, values) = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new(Self::table_name()))
				.and_where(Expr::col(Alias::new("disabled_at")).is_null())
				.and_where(
					Condition::any()
						.add(Expr::exists(sessions))
						.add(Expr::exists(runs)),
				)
				.build(PostgresQueryBuilder);
			let ids = TransactionExecutor::fetch_all(tx, &sql, convert_values(values))
				.await?
				.into_iter()
				.map(|row| row.get::<Uuid>("id").map_err(FrameworkError::from))
				.collect::<StdResult<Vec<_>, _>>()?;
			if ids.is_empty() {
				return Ok(vec![]);
			}
			Ok(Self::objects()
				.filter(Self::field_id().is_in(ids))
				.all_with_db(tx)
				.await?)
		})
		.await
	}
}

impl DashboardSession {
	pub(crate) async fn from_token<E: OrmExecutor>(
		db: &mut E,
		token_hash: Vec<u8>,
	) -> Result<Self> {
		Self::objects()
			.filter(Self::field_token_hash().eq(token_hash))
			.all_with_db(db)
			.await?
			.pop()
			.ok_or(Error::Unauthorized)
	}

	pub(crate) async fn start(
		db: DatabaseConnection,
		identity: Uuid,
		hashes: (Vec<u8>, Vec<u8>),
		provider_sid: Option<String>,
		auth_time: DateTime<Utc>,
		lifetime: i64,
		previous: Option<Vec<u8>>,
	) -> Result<()> {
		db.atomic(async |tx| {
			let identity_row = DashboardIdentity::objects()
				.filter(DashboardIdentity::field_id().eq(identity))
				.select_for_update()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or(Error::Forbidden)?;
			if identity_row.disabled_at.is_some()
				|| aidash_domain::identity::dashboard::session_revoked(
					auth_time,
					identity_row.valid_since,
				) {
				return Err(Error::Forbidden);
			}
			let now = database_time(tx).await?;
			if let Some(previous) = previous {
				Self::objects()
					.filter(Self::field_token_hash().eq(previous))
					.filter(Self::field_revoked_at().is_null())
					.update_fields_with_conn(tx, [(Self::field_revoked_at(), Some(now))])
					.await?;
			}
			let session = Self::build()
				.id(Uuid::new_v4())
				.identity_id(identity)
				.token_hash(hashes.0)
				.csrf_hash(hashes.1)
				.provider_sid(provider_sid)
				.created_at(now)
				.auth_time(Some(auth_time))
				.last_activity_at(now)
				.expires_at(now + Duration::seconds(lifetime))
				.revoked_at(None)
				.desktop(false)
				.desktop_idle_seconds(None)
				.access_expires_at(None)
				.finish();
			Self::objects().create_with_conn(tx, &session).await?;
			Ok(())
		})
		.await
	}

	pub(crate) async fn revoke(
		db: DatabaseConnection,
		id: Uuid,
		all_for_identity: bool,
	) -> Result<()> {
		db.atomic(async |tx| {
			if all_for_identity {
				let (sql, values) = Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("dashboard_identities"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.lock(LockType::Update)
					.build(PostgresQueryBuilder);
				TransactionExecutor::fetch_one(tx, &sql, convert_values(values)).await?;
			}
			let now = database_time(tx).await?;
			let mut rows = Self::objects().filter(Self::field_revoked_at().is_null());
			rows = if all_for_identity {
				rows.filter(Self::field_identity_id().eq(id))
			} else {
				rows.filter(Self::field_id().eq(id))
			};
			rows.update_fields_with_conn(tx, [(Self::field_revoked_at(), Some(now))])
				.await?;
			Ok(())
		})
		.await
	}

	/// Keep the request's browser session in the same native authority transaction.
	pub(crate) async fn require_current(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		identity: Uuid,
		idle_seconds: i64,
		access_expires_at: Option<DateTime<Utc>>,
	) -> Result<()> {
		let mut current = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("identity_id").eq(Expr::value(identity)))
			.and_where(Expr::col("revoked_at").is_null())
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col("expires_at"))
					.gt(Expr::cust("clock_timestamp()")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col("last_activity_at")).gt(Expr::cust(
					"clock_timestamp()",
				)
				.sub(
					reinhardt::query::SimpleExpr::from(Expr::value(idle_seconds))
						.mul(Expr::cust("interval '1 second'")),
				)),
			)
			.lock(LockType::Share)
			.to_owned();
		if let Some(expires) = access_expires_at {
			current.and_where(Expr::value(expires).gt(Expr::cust("clock_timestamp()")));
		}
		let (sql, values) = current.build(PostgresQueryBuilder);
		if TransactionExecutor::fetch_optional(tx, &sql, convert_values(values))
			.await?
			.is_none()
		{
			return Err(Error::Unauthorized);
		}
		Ok(())
	}

	pub(crate) async fn record_activity(db: DatabaseConnection, id: Uuid) -> Result<()> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			Self::objects()
				.filter(Self::field_id().eq(id))
				.update_fields_with_conn(tx, [(Self::field_last_activity_at(), now)])
				.await?;
			Ok(())
		})
		.await
	}
}

impl DashboardLogoutToken {
	pub(crate) async fn revoke_sessions(
		db: DatabaseConnection,
		issuer: &str,
		subject: Option<&str>,
		sid: Option<&str>,
		jti: Vec<u8>,
		expires_at: DateTime<Utc>,
	) -> Result<()> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			Self::objects()
				.filter(Self::field_expires_at().lt(now - Duration::seconds(30)))
				.delete_with_conn(tx)
				.await?;
			let (sql, values) = Query::insert()
				.into_table(Alias::new(Self::table_name()))
				.columns(["jti_hash", "expires_at"].map(Alias::new))
				.values_panic([
					IntoValue::into_value(jti),
					IntoValue::into_value(expires_at),
				])
				.on_conflict(OnConflict::columns(["jti_hash"]).do_nothing().to_owned())
				.build(PostgresQueryBuilder);
			if TransactionExecutor::execute(tx, &sql, convert_values(values))
				.await?
				.rows_affected
				!= 1
			{
				return Err(Error::Invalid("replayed logout token".into()));
			}
			let mut identities = Query::select();
			identities
				.column(Alias::new("id"))
				.from(Alias::new(DashboardIdentity::table_name()))
				.and_where(
					Expr::col(Alias::new("issuer")).eq(reinhardt::query::Expr::value(issuer)),
				);
			if let Some(subject) = subject {
				identities.and_where(
					Expr::col(Alias::new("subject")).eq(reinhardt::query::Expr::value(subject)),
				);
			}
			if let Some(session_id) = sid
				&& subject.is_none()
			{
				let sessions = Query::select()
					.column(Alias::new("identity_id"))
					.from(Alias::new("dashboard_sessions"))
					.and_where(Expr::col("provider_sid").eq(Expr::value(session_id)))
					.to_owned();
				identities.and_where(Expr::col("id").in_subquery(sessions));
			}
			let mut lock = identities.clone();
			lock.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
				.lock(LockType::Update);
			let (sql, values) = lock.build(PostgresQueryBuilder);
			TransactionExecutor::fetch_all(tx, &sql, convert_values(values)).await?;
			let mut update = Query::update();
			update
				.table(Alias::new(DashboardSession::table_name()))
				.value(Alias::new("revoked_at"), now)
				.and_where(Expr::col(Alias::new("revoked_at")).is_null())
				.and_where(Expr::col(Alias::new("identity_id")).in_subquery(identities.to_owned()));
			if let Some(sid) = sid {
				update.and_where(
					Expr::col(Alias::new("provider_sid")).eq(reinhardt::query::Expr::value(sid)),
				);
			}
			let (sql, values) = update.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			Ok(())
		})
		.await
	}
}

use reinhardt::query::IntoValue;
