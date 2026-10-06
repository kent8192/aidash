//! Policy and credential leases held on the protected native transaction.
use super::{
	AuthorizationBundle, AuthorizationCredential, AuthorizationRevision, DashboardIdentity,
	DashboardMapping,
};
use crate::apps::identity::serializers::contracts::Snapshot;
use crate::apps::identity::serializers::identity::Credential;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::OnConflict;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

impl AuthorizationBundle {
	pub(crate) async fn replace(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		expected_revision: i64,
		document: Value,
		actor: &str,
	) -> Result<i64> {
		authority_control(tx).await?;
		let (sql, values) = if expected_revision == 0 {
			Query::insert()
				.into_table(Alias::new(Self::table_name()))
				.columns(["tenant", "revision", "document"].map(Alias::new))
				.values_panic([
					IntoValue::into_value(tenant),
					IntoValue::into_value(1_i64),
					IntoValue::into_value(document.clone()),
				])
				.on_conflict(
					OnConflict::column(Alias::new("tenant"))
						.do_nothing()
						.to_owned(),
				)
				.returning([Alias::new("revision")])
				.build(PostgresQueryBuilder)
		} else {
			Query::update()
				.table(Alias::new(Self::table_name()))
				.value_expr(
					Alias::new("revision"),
					Expr::col("revision").add(Expr::value(1_i64)),
				)
				.value_expr(Alias::new("document"), Expr::value(document.clone()))
				.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
				.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
				.and_where(Expr::col("revision").eq(Expr::value(expected_revision)))
				.returning([Alias::new("revision")])
				.build(PostgresQueryBuilder)
		};
		let row = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.ok_or_else(|| Error::Conflict("authorization revision changed".into()))?;
		let revision: i64 = row.get("revision").map_err(FrameworkError::from)?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new(AuthorizationRevision::table_name()))
			.columns(["tenant", "revision", "document", "actor"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(tenant),
				IntoValue::into_value(revision),
				IntoValue::into_value(document),
				IntoValue::into_value(actor),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(revision)
	}

	pub(crate) async fn lock_snapshot(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		exclusive: bool,
	) -> Result<Snapshot> {
		let (sql, values) = Query::select()
			.columns(["revision", "document"].map(Alias::new))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.lock(if exclusive {
				LockType::Update
			} else {
				LockType::Share
			})
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.ok_or_else(|| Error::NotFound("authorization policy".into()))?;
		let row = QueryRow::from_backend_row(row).data;
		Ok(Snapshot {
			revision: serde_json::from_value(row["revision"].clone())?,
			bundle: serde_json::from_value(row["document"].clone())?,
		})
	}
}

impl AuthorizationCredential {
	pub(crate) async fn issue(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		subject: &str,
		token_hash: Vec<u8>,
		lifetime: i64,
		actor: &str,
	) -> Result<Credential> {
		authority_control(tx).await?;
		let (sql, values) = Query::select()
			.expr_as(
				SimpleExpr::FunctionCall("clock_timestamp".into_iden(), vec![]),
				Alias::new("now"),
			)
			.build(PostgresQueryBuilder);
		let now: DateTime<Utc> = tx
			.fetch_one(&sql, convert_values(values))
			.await?
			.get("now")
			.map_err(FrameworkError::from)?;
		let record = Credential {
			id: Uuid::new_v4(),
			tenant: tenant.into(),
			subject: subject.into(),
			created_at: now,
			expires_at: now + Duration::seconds(lifetime),
			revoked_at: None,
			issued_by: actor.into(),
		};
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(
				[
					"id",
					"tenant",
					"subject",
					"token_hash",
					"created_at",
					"expires_at",
					"issued_by",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(record.id),
				IntoValue::into_value(tenant),
				IntoValue::into_value(subject),
				IntoValue::into_value(token_hash),
				IntoValue::into_value(record.created_at),
				IntoValue::into_value(record.expires_at),
				IntoValue::into_value(actor),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(record)
	}

	pub(crate) async fn authenticate(
		tx: &mut dyn TransactionExecutor,
		token_hash: Vec<u8>,
	) -> Result<(Uuid, String, String)> {
		let (sql, values) = Query::select()
			.columns(["id", "tenant", "subject"].map(Alias::new))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("token_hash").eq(Expr::value(token_hash)))
			.and_where(Expr::col("revoked_at").is_null())
			.and_where(Expr::col("expires_at").gt(SimpleExpr::FunctionCall(
				"clock_timestamp".into_iden(),
				vec![],
			)))
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.ok_or(Error::Unauthorized)?;
		Ok((
			row.get("id").map_err(FrameworkError::from)?,
			row.get("tenant").map_err(FrameworkError::from)?,
			row.get("subject").map_err(FrameworkError::from)?,
		))
	}

	pub(crate) async fn lock_valid(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		tenant: &str,
		subject: &str,
	) -> Result<()> {
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("subject").eq(Expr::value(subject)))
			.and_where(Expr::col("revoked_at").is_null())
			.and_where(Expr::col("expires_at").gt(SimpleExpr::FunctionCall(
				"clock_timestamp".into_iden(),
				vec![],
			)))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_none()
		{
			return Err(Error::Unauthorized);
		}
		let (sql, values) = Query::select()
			.columns(["identity_id", "enabled"].map(Alias::new))
			.from(Alias::new(DashboardMapping::table_name()))
			.and_where(Expr::col("credential_id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		if let Some(mapping) = tx.fetch_optional(&sql, convert_values(values)).await? {
			if !mapping
				.get::<bool>("enabled")
				.map_err(FrameworkError::from)?
			{
				return Err(Error::Forbidden);
			}
			let identity: Uuid = mapping.get("identity_id").map_err(FrameworkError::from)?;
			let (sql, values) = Query::select()
				.columns(["issuer", "last_valid_at", "disabled_at"].map(Alias::new))
				.from(Alias::new(DashboardIdentity::table_name()))
				.and_where(Expr::col("id").eq(Expr::value(identity)))
				.lock(LockType::Share)
				.build(PostgresQueryBuilder);
			let identity = tx
				.fetch_optional(&sql, convert_values(values))
				.await?
				.ok_or(Error::Forbidden)?;
			let identity = QueryRow::from_backend_row(identity).data;
			let disabled: Option<DateTime<Utc>> =
				serde_json::from_value(identity["disabled_at"].clone())?;
			let valid: Option<DateTime<Utc>> =
				serde_json::from_value(identity["last_valid_at"].clone())?;
			let issuer = serde_json::from_value(identity["issuer"].clone())?;
			crate::authorization::identity::validate_dashboard_status(Some((
				issuer, valid, disabled,
			)))?;
		}
		Ok(())
	}
}

pub(crate) async fn authority_control(tx: &mut dyn TransactionExecutor) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"set_config".into_iden(),
			vec![
				Expr::value("aidash.transaction_control").into(),
				Expr::value("authority").into(),
				Expr::value(true).into(),
			],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

use reinhardt::query::IntoValue;
