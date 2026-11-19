//! Stored policies after Schedules became Duties (#1011) and Agents may form
//! Subnodes as ordinary members (#1012): `schedule.create` and
//! `schedule.administer` become `duty.create` and `duty.administer`
//! everywhere, and the unchanged default member policy gains
//! `subnode.create`. A policy someone edited keeps what they chose.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

const RENAMES: [(&str, &str); 2] = [
	("schedule.create", "duty.create"),
	("schedule.administer", "duty.administer"),
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

fn subnode_member_policy() -> String {
	let bundle = aidash_domain::identity::tenancy::initial_policy("migration", false)
		.expect("the migration Tenant name is valid");
	let current = bundle
		.policies
		.into_iter()
		.find(|policy| policy.id == "members")
		.expect("the initial member policy exists");
	// Its text predates the Thread actions (#1008), which have their own migration.
	let current = crate::apps::identity::services::thread_policy_backfill::before_threads(current);
	let mut previous = current.clone();
	previous
		.actions
		.remove(aidash_domain::identity::tenant_administration::SUBNODE_CREATE);
	let previous = serde_json::to_value(previous).expect("the member policy serializes");
	let current = serde_json::to_value(current).expect("the member policy serializes");
	// Reinhardt Query has no builder for PostgreSQL's JSONB array transformation.
	let policies = SimpleExpr::CustomWithExpr(
		"jsonb_set(document, '{policies}', (SELECT jsonb_agg(CASE WHEN entry = ?::jsonb THEN ?::jsonb ELSE entry END ORDER BY ord) FROM jsonb_array_elements(document->'policies') WITH ORDINALITY AS p(entry, ord)))".into(),
		vec![Expr::value(previous.clone()).into(), Expr::value(current).into()],
	);
	let matches_default = SimpleExpr::CustomWithExpr(
		"document->'policies' @> jsonb_build_array(?::jsonb)".into(),
		vec![Expr::value(previous).into()],
	);
	Query::update()
		.table(Alias::new("authorization_bundles"))
		.value_expr(Alias::new("document"), policies)
		.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(matches_default)
		.to_string(PostgresQueryBuilder)
}

pub(crate) fn forward() -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n{};\n",
		rename(|(from, to)| (from.to_owned(), to.to_owned())),
		subnode_member_policy()
	)
}

/// Reversal names the Schedule actions again and leaves `subnode.create`,
/// since later policy edits cannot be told apart from this one.
pub(crate) fn backward() -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n",
		rename(|(from, to)| (to.to_owned(), from.to_owned()))
	)
}
