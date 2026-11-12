//! The root record of every Tenant (ADR 0050). It is Deployment-wide and is
//! never deleted: purge clears only the label, so a Tenant name is never reused.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "tenants")]
#[derive(Serialize, Deserialize)]
pub struct Tenant {
	#[field(primary_key = true, field_type = "text")]
	pub name: String,
	/// The Tenant label; absent only after purge or for Tenants that predate it.
	#[field(field_type = "text", null = true)]
	pub label: Option<String>,
	#[field(default = false)]
	pub personal: bool,
	/// An optional Tenant ceiling, in seconds, below the Deployment maximum.
	#[field(null = true)]
	pub credential_max_seconds: Option<i64>,
	#[field]
	pub created_at: DateTime<Utc>,
	/// `identity` for a Tenant Creator, `deployment_administrator` for a Deployment Administrator.
	#[field(field_type = "text")]
	pub created_by_kind: String,
	/// The creating External Identity: the Tenant Creator, or the Deployment
	/// Administrator's own identity when one acted through a browser session.
	#[field(null = true)]
	pub created_by_identity: Option<uuid::Uuid>,
}

use crate::Result;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

impl Tenant {
	/// Serialize lifetime changes with issuance, including when no override exists.
	pub(crate) async fn credential_maximum(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		deployment: i64,
		exclusive: bool,
	) -> Result<i64> {
		let (sql, values) = Query::select()
			.column(Alias::new("credential_max_seconds"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("name").eq(tenant.to_owned()))
			.lock(if exclusive {
				LockType::Update
			} else {
				LockType::Share
			})
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.ok_or(crate::Error::Forbidden)?;
		// A NULL maximum reads as `None` through `QueryRow`: the Deployment's applies.
		let configured: Option<i64> =
			reinhardt::db::orm::QueryRow::from_backend_row(row).get("credential_max_seconds");
		Ok(configured.unwrap_or(deployment).min(deployment))
	}
	/// The name of every Tenant, oldest first.
	pub(crate) async fn names<E: reinhardt::db::orm::OrmExecutor>(
		db: &mut E,
	) -> Result<Vec<aidash_domain::tenancy::TenantName>> {
		Self::objects()
			.all()
			.order_by(&["created_at", "name"])
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|tenant| Ok(aidash_domain::tenancy::TenantName::new(tenant.name)?))
			.collect()
	}
	/// Whether `tenant` has a root record, held against deletion until the
	/// transaction ends.
	pub(crate) async fn exists(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<bool> {
		let (sql, values) = Query::select()
			.column(Alias::new("name"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("name").eq(Expr::value(tenant)))
			.lock(LockType::KeyShare)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some())
	}
}
