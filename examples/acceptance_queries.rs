//! SQL for the Python acceptance harness, generated with the same query builder
//! as the application. This helper does not connect to a database.
use sea_orm::sea_query::{Alias, Asterisk, Expr, PostgresQueryBuilder, Query};
fn evidence(table: &str, columns: &[&str], predicate: &str) -> String {
	let mut row = Query::select();
	row.columns(columns.iter().map(|c| Alias::new(*c)))
		.from(Alias::new(table))
		.and_where(Expr::cust(predicate));
	Query::select()
		.expr(
			sea_orm::sea_query::Func::cust(Alias::new("row_to_json"))
				.arg(Expr::col(Alias::new("e"))),
		)
		.from_subquery(row.to_owned(), Alias::new("e"))
		.to_string(PostgresQueryBuilder)
}
fn main() {
	let pending = Query::select()
		.expr(Expr::col(Asterisk).count())
		.from(Alias::new("events"))
		.and_where(Expr::col(Alias::new("published_at")).is_null())
		.to_string(PostgresQueryBuilder);
	let inbox = Query::select()
		.expr(Expr::col(Asterisk).count())
		.from(Alias::new("inbox"))
		.to_string(PostgresQueryBuilder);
	let count = |table: &str| {
		Query::select()
			.expr(Expr::col(Asterisk).count())
			.from(Alias::new(table))
			.to_string(PostgresQueryBuilder)
	};
	println!(
		"{}",
		serde_json::json!({"pending_events":pending,"inbox":inbox,
			"tx_authority_control":Query::select().expr(Expr::cust("set_config('aidash.transaction_control','authority',true)")).to_string(PostgresQueryBuilder),
			"tx_disable_peer":Query::update().table(Alias::new("peers")).value(Alias::new("enabled"),false).and_where(Expr::col(Alias::new("node_id")).eq("aidash://tx-01")).to_string(PostgresQueryBuilder),
			"tx_peer":evidence("peers", &["node_id","enabled"], "node_id='aidash://tx-01'"),
			"tx_coordinator":evidence("atomic_coordinators",&["id","digest","decision","visible","complete"],"id=:'transaction'::uuid"),
			"tx_participant":evidence("atomic_participants",&["id","digest","phase"],"id=:'transaction'::uuid"),
			"tx_barrier":evidence("atomic_gate",&["transaction_id","commit_epoch"],"singleton"),
			"tx_workspace":evidence("workspaces",&["id","revision"],"id=:'workspace'::uuid"),
			"tx_events":evidence("events",&["id","kind"],"workspace_id=:'workspace'::uuid AND kind='workspace.updated'"),
			"remote_admissions":count("authorization_remote_admissions"),
			"remote_bindings":count("authorization_remote_execution")})
	);
}
