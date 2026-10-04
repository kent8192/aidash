//! Persistent authorization_decisions records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_decisions")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationDecision {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text")]
	pub action: String,
	#[field(field_type = "text")]
	pub resource_kind: String,
	#[field(field_type = "text")]
	pub resource_id: String,
	#[field]
	pub decision: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

use crate::Result;
use crate::apps::identity::services::policy::{Decision, Evaluation};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;

impl AuthorizationDecision {
	pub(crate) async fn append(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		super::authority::authority_control(tx).await?;
		// The cursor follows commit order, including concurrently admitted inputs.
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![Expr::value(71003202_i64).into()],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new("authorization_decisions"))
			.columns([
				Alias::new("tenant"),
				Alias::new("revision"),
				Alias::new("subject"),
				Alias::new("action"),
				Alias::new("resource_kind"),
				Alias::new("resource_id"),
				Alias::new("decision"),
			])
			.values_panic([
				IntoValue::into_value(tenant),
				IntoValue::into_value(decision.revision),
				IntoValue::into_value(&input.subject),
				IntoValue::into_value(&input.action),
				IntoValue::into_value(&input.resource.kind),
				IntoValue::into_value(&input.resource.id),
				IntoValue::into_value(serde_json::to_value(decision)?),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
	pub(crate) async fn page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		after: i64,
		limit: i64,
	) -> Result<Vec<Value>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_sequence().gt(after))
			.order_by(&["sequence"])
			.limit(limit.clamp(1, 200) as usize)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| {
				json!({
					"sequence": row.sequence, "tenant": row.tenant, "revision": row.revision,
					"subject": row.subject, "action": row.action, "resource_kind": row.resource_kind,
					"resource_id": row.resource_id, "decision": row.decision.0, "created_at": row.created_at,
				})
			})
			.collect())
	}
}

use reinhardt::query::IntoValue;
