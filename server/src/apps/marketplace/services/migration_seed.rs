//! Query-generated baseline Marketplace compatibility gate.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, Value,
};

pub(crate) fn forward() -> String {
	Query::insert()
		.into_table((Alias::new("public"), Alias::new("marketplace_gate")))
		.columns([Alias::new("key"), Alias::new("document")])
		.values_panic([
			"v1".into(),
			Value::Json(Some(Box::new(serde_json::json!({
				"enabled": false, "contract": 1, "revision": 1
			})))),
		])
		.to_string(PostgresQueryBuilder)
}

pub(crate) fn backward() -> String {
	Query::delete()
		.from_table((Alias::new("public"), Alias::new("marketplace_gate")))
		.and_where(Expr::col(Alias::new("key")).eq("v1"))
		.to_string(PostgresQueryBuilder)
}
