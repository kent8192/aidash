//! Stored policies after the Workspace was dissolved into Threads (#1008,
//! ADR 0140): the `workspace.*` actions become `thread.*`, the `workspace`
//! resource kind becomes `thread`, and the `workspace_id` attribute a
//! condition reads (`/workspace_id`) becomes `thread_id`, in every policy of
//! every Tenant.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

/// The first five are actions (see `before_threads`); a condition names an
/// attribute by its JSON Pointer, so both the bare name and the pointer go.
const RENAMES: [(&str, &str); 8] = [
	("workspace.create", "thread.create"),
	("workspace.events", "thread.events"),
	("workspace.read", "thread.read"),
	("workspace.share", "thread.share"),
	("workspace.update", "thread.update"),
	("workspace_id", "thread_id"),
	("/workspace_id", "/thread_id"),
	("workspace", "thread"),
];

fn rename(from_to: impl Fn((&str, &str)) -> (String, String)) -> String {
	let mut document = "document::text".to_owned();
	let mut values = Vec::new();
	for pair in RENAMES {
		let (from, to) = from_to(pair);
		document = format!("replace({document}, ?, ?)");
		values.push(Expr::value(format!("\"{from}\"")).into());
		values.push(Expr::value(format!("\"{to}\"")).into());
	}
	let mut matched = Vec::new();
	let mut any = Vec::new();
	for pair in RENAMES {
		let (from, _) = from_to(pair);
		matched.push("document::text LIKE ?".to_owned());
		any.push(Expr::value(format!("%\"{from}\"%")).into());
	}
	Query::update()
		.table(Alias::new("authorization_bundles"))
		.value_expr(
			Alias::new("document"),
			SimpleExpr::CustomWithExpr(format!("({document})::jsonb"), values),
		)
		.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(SimpleExpr::CustomWithExpr(
			format!("({})", matched.join(" OR ")),
			any,
		))
		.to_string(PostgresQueryBuilder)
}

pub(crate) fn forward() -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n",
		rename(|(from, to)| (from.to_owned(), to.to_owned()))
	)
}

/// Reversal names the Workspace actions, kind and attribute again.
pub(crate) fn backward() -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n",
		rename(|(from, to)| (to.to_owned(), from.to_owned()))
	)
}

/// The default member policy as it was stored before #1008, for the backfills
/// of earlier migrations whose text must not change: they compare stored
/// policies with the member policy of their time.
pub(crate) fn before_threads(
	mut policy: aidash_domain::policy::Policy,
) -> aidash_domain::policy::Policy {
	for (from, to) in RENAMES.iter().take(5) {
		if policy.actions.remove(*to) {
			policy.actions.insert((*from).to_owned());
		}
	}
	policy
}
