//! A Tenant's execution settings (#1009). A Tenant that never stored them uses
//! the defaults at revision 0.
use crate::{Error, Result, database::native};
use aidash_domain::run_state::{LEASE_RECOVERY_LIMIT, LEASE_RECOVERY_LIMITS};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

const TABLE: &str = "tenant_execution_settings";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
	pub lease_recovery_limit: u32,
	pub revision: i64,
	pub updated_at: Option<DateTime<Utc>>,
	pub updated_by: Option<String>,
}

impl Default for Stored {
	fn default() -> Self {
		Self {
			lease_recovery_limit: LEASE_RECOVERY_LIMIT,
			revision: 0,
			updated_at: None,
			updated_by: None,
		}
	}
}

async fn lock_tenant(tx: &mut dyn TransactionExecutor, tenant: &str, lock: LockType) -> Result<()> {
	let query = Query::select()
		.column(Alias::new("name"))
		.from(Alias::new("tenants"))
		.and_where(Expr::col("name").eq(tenant))
		.lock(lock)
		.to_string(PostgresQueryBuilder);
	if native::query(&query)
		.fetch_optional(&mut *tx)
		.await?
		.is_none()
	{
		return Err(Error::NotFound("tenant not found".into()));
	}
	Ok(())
}

async fn read(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Stored> {
	let query = Query::select()
		.columns(
			[
				"lease_recovery_limit",
				"revision",
				"updated_at",
				"updated_by",
			]
			.map(Alias::new),
		)
		.from(Alias::new(TABLE))
		.and_where(Expr::col("tenant").eq(tenant))
		.to_string(PostgresQueryBuilder);
	Ok(match native::query(&query).fetch_optional(tx).await? {
		Some(row) => Stored {
			lease_recovery_limit: u32::try_from(row.try_get::<i32>("lease_recovery_limit")?)
				.unwrap_or(LEASE_RECOVERY_LIMIT),
			revision: row.try_get("revision")?,
			updated_at: Some(row.try_get("updated_at")?),
			updated_by: Some(row.try_get("updated_by")?),
		},
		None => Stored::default(),
	})
}

pub(crate) async fn get(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Stored> {
	lock_tenant(tx, tenant, LockType::KeyShare).await?;
	read(tx, tenant).await
}

/// Replaces the settings when `expected` is still the current revision.
pub(crate) async fn set(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	lease_recovery_limit: u32,
	expected: i64,
	by: &str,
) -> Result<i64> {
	if !LEASE_RECOVERY_LIMITS.contains(&lease_recovery_limit) {
		return Err(Error::Invalid(format!(
			"lease_recovery_limit must be from {} to {}",
			LEASE_RECOVERY_LIMITS.start(),
			LEASE_RECOVERY_LIMITS.end()
		)));
	}
	lock_tenant(tx, tenant, LockType::Update).await?;
	let current = read(tx, tenant).await?.revision;
	if current != expected || current == i64::MAX {
		return Err(Error::Conflict(
			"execution settings revision changed".into(),
		));
	}
	let revision = current + 1;
	native::query(
		&Query::insert()
			.into_table(Alias::new(TABLE))
			.columns(
				[
					"tenant",
					"lease_recovery_limit",
					"revision",
					"updated_at",
					"updated_by",
				]
				.map(Alias::new),
			)
			.values_panic([
				Expr::value(tenant),
				Expr::value(lease_recovery_limit as i32),
				Expr::value(revision),
				Expr::value(Utc::now()),
				Expr::value(by),
			])
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.update_columns(
						[
							"lease_recovery_limit",
							"revision",
							"updated_at",
							"updated_by",
						]
						.map(Alias::new),
					)
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(tx)
	.await?;
	Ok(revision)
}
