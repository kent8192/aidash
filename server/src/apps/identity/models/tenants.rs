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
	#[field]
	pub created_at: DateTime<Utc>,
	/// `identity` for a Tenant Creator, `operator` for a Deployment Administrator.
	#[field(field_type = "text")]
	pub created_by_kind: String,
	/// The creating External Identity: the Tenant Creator, or the Deployment
	/// Administrator's own identity when one acted through a browser session.
	#[field(null = true)]
	pub created_by_identity: Option<uuid::Uuid>,
}

use crate::Result;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

impl Tenant {
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
			.await
			.map_err(FrameworkError::from)?
			.is_some())
	}
}
