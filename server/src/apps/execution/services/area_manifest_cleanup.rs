//! Discard Working Area manifests and queue their unreferenced objects for reclamation.
use reinhardt::query::{
	Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

pub(crate) fn forward() -> String {
	let references = Query::delete()
		.from_table("core_blob_references")
		.and_where(Expr::col("holder_kind").eq("revision"))
		.to_string(PostgresQueryBuilder);
	let objects = Query::update()
		.table("core_objects")
		.value("kind", "superseded_working")
		.and_where(Expr::col("kind").is_in(["working", "received"]))
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{references};\n{objects};\n")
}
