//! Read-only Reinhardt Query diagnostics for Request Estimate calibration
//! (ADR 0017). Deployment Administrators see Deployment-wide aggregates per calibration key;
//! samples carry no Run, Tenant or attempt identifier, and none is selected.
use aidash_domain::provider::estimate::{CALIBRATION_MINIMUM, PPM};
use reinhardt::query::{
	Alias, Expr, IntoIden, Order, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

/// One row per `(upstream model, Projection Version)`: the sample count, the
/// nearest-rank 99th-percentile ratio in parts per million (capped at one token
/// per byte, as the estimator does), whether the key has enough samples to
/// calibrate, and the newest sample time.
pub(super) fn pools() -> String {
	let ratio = format!("LEAST(CEIL(prompt_tokens * {PPM}.0 / byte_basis), {PPM})::bigint");
	let mut row = Query::select();
	row.column(Alias::new("upstream_model"))
		.column(Alias::new("projection_version"))
		.expr_as(Expr::cust("COUNT(*)"), Alias::new("samples"))
		.expr_as(
			Expr::cust(format!(
				"PERCENTILE_DISC(0.99) WITHIN GROUP (ORDER BY {ratio})"
			)),
			Alias::new("ratio_ppm"),
		)
		.expr_as(
			Expr::cust(format!("COUNT(*) >= {CALIBRATION_MINIMUM}")),
			Alias::new("calibrated"),
		)
		.expr_as(Expr::cust("MAX(completed_at)"), Alias::new("latest"))
		.from(Alias::new("estimate_calibration_samples"))
		.group_by_col(Alias::new("upstream_model"))
		.group_by_col(Alias::new("projection_version"))
		.order_by(Alias::new("upstream_model"), Order::Asc)
		.order_by(Alias::new("projection_version"), Order::Asc);
	Query::select()
		.expr(SimpleExpr::FunctionCall(
			"row_to_json".into_iden(),
			vec![Expr::col(Alias::new("e")).into()],
		))
		.from_subquery(row.to_owned(), Alias::new("e"))
		.to_string(PostgresQueryBuilder)
}

pub(super) fn document() -> serde_json::Value {
	serde_json::json!({ "calibration_pools": pools() })
}
