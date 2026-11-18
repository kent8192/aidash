//! Delete the Catalog Approvals of the removed package indexes `core.npm` and `core.pypi`
//! (#745, #729). The rendered statements are the `identity/0082_remove_package_index_approvals`
//! SQL assets, checked by `manage migrationseeds --check`.

/// The removed built-in package indexes.
pub(crate) const REMOVED: [&str; 2] = ["core.npm", "core.pypi"];

pub(crate) fn forward() -> String {
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	Query::delete()
		.from_table(Alias::new("authorization_catalog"))
		.and_where(Expr::col("entry_id").is_in(REMOVED))
		.to_string(PostgresQueryBuilder)
}

/// Reversal does not restore the approvals: nothing serves the removed indexes.
pub(crate) fn backward() -> String {
	use reinhardt::query::{Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
	format!(
		"{};\n",
		Query::select()
			.expr(Expr::value(1))
			.to_string(PostgresQueryBuilder)
	)
}
