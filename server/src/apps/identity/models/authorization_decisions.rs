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
	Alias, Expr, ExprTrait, IntoIden, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::json;

impl AuthorizationDecision {
	pub(crate) async fn append(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		Self::append_many(tx, tenant, &[(input.clone(), decision.clone())]).await
	}

	/// Keep every decision in evaluation order without a round trip per check.
	pub(crate) async fn append_many(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		records: &[(Evaluation, Decision)],
	) -> Result<()> {
		if records.is_empty() {
			return Ok(());
		}
		super::authority::authority_control(tx).await?;
		// Acquire before any INSERT can allocate a sequence, and retain through
		// commit so a cursor never skips a concurrently admitted transaction.
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![Expr::value(71003202_i64).into()],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		let columns = [
			"tenant",
			"revision",
			"subject",
			"action",
			"resource_kind",
			"resource_id",
			"decision",
		];
		// Bound statement size and parameter count for large disclosure responses.
		for records in records.chunks(100) {
			let mut rows = Query::select();
			for (index, (input, decision)) in records.iter().enumerate() {
				let mut row = Query::select();
				// cast_as quotes type names; bigint's catalog name is int8.
				for (name, value) in [
					("tenant", Expr::value(tenant).cast_as("text")),
					("revision", Expr::value(decision.revision).cast_as("int8")),
					("subject", Expr::value(&input.subject).cast_as("text")),
					("action", Expr::value(&input.action).cast_as("text")),
					(
						"resource_kind",
						Expr::value(&input.resource.kind).cast_as("text"),
					),
					(
						"resource_id",
						Expr::value(&input.resource.id).cast_as("text"),
					),
					(
						"decision",
						Expr::value(serde_json::to_value(decision)?).cast_as("jsonb"),
					),
				] {
					row.expr_as(value, Alias::new(name));
				}
				row.expr_as(
					Expr::value(i64::try_from(index).expect("bounded audit batch")).cast_as("int8"),
					Alias::new("ordinal"),
				);
				if index == 0 {
					rows = row;
				} else {
					rows.union_all(row);
				}
			}
			let (sql, values) = Query::insert()
				.into_table(Alias::new("authorization_decisions"))
				.columns(columns.map(Alias::new))
				.from_subquery(
					Query::select()
						.columns(columns.map(|name| (Alias::new("decisions"), Alias::new(name))))
						.from_subquery(rows, Alias::new("decisions"))
						.order_by((Alias::new("decisions"), Alias::new("ordinal")), Order::Asc)
						.to_owned(),
				)
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
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
