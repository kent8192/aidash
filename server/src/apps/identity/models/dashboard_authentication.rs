//! Browser authentication state and provider-status persistence.

use super::dashboard_administration::database_time;
use super::{DashboardIdentity, DashboardLoginTransaction, DashboardLogoutToken, DashboardSession};
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
		subject: &str,
	) -> Result<Self> {
		db.atomic(async |tx| {
			let now = database_time(tx).await?;
			let (sql, values) = Query::insert()
				.into_table(Alias::new(Self::table_name()))
				.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
				.values_panic([
					IntoValue::into_value(issuer),
					IntoValue::into_value(subject),
					IntoValue::into_value(now),
				])
				.on_conflict(
					OnConflict::columns(["issuer", "subject"])
						.do_nothing()
						.to_owned(),
				)
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
			Self::objects()
				.filter(Self::field_issuer().eq(issuer))
				.filter(Self::field_subject().eq(subject))
				.all_with_db(tx)
				.await?
				.pop()
				.ok_or_else(|| Error::NotFound("identity".into()))
		})
		.await
	}

	pub(crate) async fn record_valid(
		db: DatabaseConnection,
		id: Uuid,
		checked_at: DateTime<Utc>,
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
			Self::objects().update_with_conn(tx, &row).await?;
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
						.gt(Expr::value(now - Duration::seconds(idle_seconds))),
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
		lifetime: i64,
		previous: Option<Vec<u8>>,
	) -> Result<()> {
		db.atomic(async |tx| {
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
				.last_activity_at(now)
				.expires_at(now + Duration::seconds(lifetime))
				.revoked_at(None)
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
	) -> Result<()> {
		let (sql, values) = Query::select()
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
			.build(PostgresQueryBuilder);
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
