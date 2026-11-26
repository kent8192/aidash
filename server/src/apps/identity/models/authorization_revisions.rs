//! Persistent authorization_revisions records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_revisions")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRevision {
	#[field(field_type = "text", primary_key = true)]
	pub tenant: String,
	#[field(primary_key = true)]
	pub revision: i64,
	#[field]
	pub document: Json<Value>,
	#[field(field_type = "text")]
	pub actor: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	/// The Mapping a Tenant Administrator replaced the policy through.
	#[field(null = true)]
	pub actor_mapping_id: Option<uuid::Uuid>,
}

use crate::Result;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::json;
use std::collections::BTreeSet;

impl AuthorizationRevision {
	/// Reads the subject names recorded under the caller's Tenant policy lock.
	pub(crate) async fn subject_names(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
	) -> Result<BTreeSet<String>> {
		let subjects = SimpleExpr::FunctionCall(
			"jsonb_extract_path".into_iden(),
			vec![Expr::col("document").into(), Expr::value("subjects").into()],
		);
		let (sql, values) = Query::select()
			.distinct()
			.expr_as(
				SimpleExpr::FunctionCall("jsonb_object_keys".into_iden(), vec![subjects]),
				Alias::new("subject"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.build(PostgresQueryBuilder);
		tx.fetch_all(&sql, convert_values(values))
			.await?
			.iter()
			.map(|row| {
				row.get("subject")
					.map_err(reinhardt::core::exception::Error::from)
					.map_err(Into::into)
			})
			.collect()
	}

	pub(crate) async fn page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		after: i64,
		limit: i64,
	) -> Result<Vec<Value>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_revision().gt(after))
			.order_by(&["revision"])
			.limit(limit.clamp(1, 200) as usize)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| {
				json!({
					"tenant": row.tenant, "revision": row.revision, "document": row.document.0,
					"actor": row.actor, "actor_mapping_id": row.actor_mapping_id,
					"created_at": row.created_at,
				})
			})
			.collect())
	}
}

impl AuthorizationRevision {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
