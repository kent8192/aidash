//! Bounded projections of durable execution records for a remote home node.
use super::invocations;
use crate::Result;
use crate::apps::execution::serializers::runs::PeerObservation;
use crate::domain::run_state::{RawRun, RunMetadata};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{OrmExecutor, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	TableRef,
};

pub(crate) async fn observe<E: OrmExecutor>(
	db: &mut E,
	node: &str,
	home: &str,
) -> Result<PeerObservation> {
	let (sql, values) = Query::select()
		.columns([
			Alias::new("id"),
			Alias::new("task_id"),
			Alias::new("workspace_id"),
			Alias::new("home_node"),
			Alias::new("agent_id"),
			Alias::new("agent_version"),
			Alias::new("phase"),
			Alias::new("control"),
			Alias::new("step"),
			Alias::new("revision"),
			Alias::new("observed_input_seq"),
			Alias::new("ledger_worker_ready"),
			Alias::new("lease_owner"),
			Alias::new("lease_until"),
			Alias::new("updated_at"),
		])
		.expr_as(Expr::cust("'{}'::jsonb"), Alias::new("context"))
		.expr_as(Expr::cust("'{}'::jsonb"), Alias::new("pending"))
		.expr_as(Expr::cust("left(error,1024)"), Alias::new("error"))
		.from(Alias::new("runs"))
		.and_where(Expr::col(Alias::new("home_node")).eq(Expr::value(home)))
		.order_by(Alias::new("updated_at"), Order::Desc)
		.order_by(Alias::new("id"), Order::Asc)
		.limit(100)
		.build(PostgresQueryBuilder);
	let runs = db
		.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| {
			let mut data = QueryRow::from_backend_row(row).data;
			// A redacted observation is inspectable, never executable. Match
			// the typed-Run inspection boundary used by the development source.
			serde_json::from_value::<RunMetadata>(data.clone()).map(|metadata| {
				RawRun {
					metadata,
					context: data["context"].take(),
					pending: data["pending"].take(),
				}
				.inspect()
			})
		})
		.collect::<std::result::Result<_, _>>()?;
	let (sql, values) = Query::select()
		.columns([
			(Alias::new("h"), Alias::new("answered_by")),
			(Alias::new("h"), Alias::new("id")),
			(Alias::new("h"), Alias::new("workspace_id")),
			(Alias::new("h"), Alias::new("run_id")),
			(Alias::new("h"), Alias::new("kind")),
			(Alias::new("h"), Alias::new("created_at")),
		])
		.expr_as(Expr::cust("left(h.prompt,1024)"), Alias::new("prompt"))
		.expr_as(Expr::cust("CASE WHEN octet_length(h.response::text)>1024 THEN jsonb_build_object('truncated',true,'preview',left(h.response::text,1024)) ELSE h.response END"), Alias::new("response"))
		.from_as(Alias::new("human_requests"), Alias::new("h"))
		.join(JoinType::InnerJoin, TableRef::table_alias(Alias::new("runs"), Alias::new("r")),
			reinhardt::query::SimpleExpr::from(Expr::col((Alias::new("r"), Alias::new("id")))).eq(Expr::col((Alias::new("h"), Alias::new("run_id")))))
		.and_where(Expr::col((Alias::new("r"), Alias::new("home_node"))).eq(Expr::value(home)))
		.order_by((Alias::new("h"), Alias::new("created_at")), Order::Desc)
		.order_by((Alias::new("h"), Alias::new("id")), Order::Asc)
		.limit(100)
		.build(PostgresQueryBuilder);
	let human_requests = db
		.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
		.collect::<std::result::Result<_, _>>()?;
	let (sql, values) = invocations::summary(Some("i"))
		.from_as(Alias::new("invocations"), Alias::new("i"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("runs"), Alias::new("r")),
			reinhardt::query::SimpleExpr::from(Expr::col((Alias::new("r"), Alias::new("id"))))
				.eq(Expr::col((Alias::new("i"), Alias::new("run_id")))),
		)
		.and_where(Expr::col((Alias::new("r"), Alias::new("home_node"))).eq(Expr::value(home)))
		.order_by((Alias::new("i"), Alias::new("created_at")), Order::Desc)
		.order_by((Alias::new("i"), Alias::new("idempotency_key")), Order::Asc)
		.limit(100)
		.build(PostgresQueryBuilder);
	let invocations = db
		.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
		.collect::<std::result::Result<_, _>>()?;
	Ok(PeerObservation {
		node_id: node.to_owned(),
		runs,
		human_requests,
		invocations,
	})
}
