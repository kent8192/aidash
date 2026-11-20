//! Data the Thread migrations rewrite (#1008, ADR 0140), as Reinhardt Query
//! output materialized by `migrationseeds`. They run before the columns are
//! renamed, so they name `workspace_id`.
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

const DEPLOYMENT_SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";

/// The Area that survives the merge of `area`'s key: the first active or
/// running one, else the first retained or recoverable one, else any; the
/// newest first, by the creation of the last Run admitted into it (Areas keep
/// no creation time), then the highest id.
fn survivor(area: &str) -> String {
	format!(
		"(SELECT s.id FROM core_areas AS s WHERE s.tenant = {area}.tenant AND s.home_node = {area}.home_node AND s.workspace_id = {area}.workspace_id AND s.agent_id = {area}.agent_id AND s.owner = {area}.owner ORDER BY CASE WHEN s.state IN ('active', 'running') THEN 0 WHEN s.state IN ('retained', 'recoverable') THEN 1 ELSE 2 END, (SELECT max(r.created_at) FROM core_runs AS q JOIN runs AS r ON r.id = q.run_id WHERE q.area_id = s.id) DESC NULLS LAST, s.id DESC LIMIT 1)"
	)
}

/// The survivor of the Area named by `column`.
fn survivor_of(column: &str) -> SimpleExpr {
	Expr::cust(format!(
		"(SELECT {} FROM core_areas AS a WHERE a.id = {column})",
		survivor("a")
	))
	.into()
}

/// Whether the Area named by `column` is merged into another.
fn merged(column: &str) -> SimpleExpr {
	Expr::cust(format!(
		"EXISTS (SELECT 1 FROM core_areas AS a WHERE a.id = {column} AND a.id <> {})",
		survivor("a")
	))
	.into()
}

/// The tables that name an Area without a key to it, so they would name a
/// deleted Area after the merge. The rows keyed to an Area (its manifests,
/// retention holds and Live previews) go with it.
const AREA_HOLDERS: [&str; 6] = [
	"core_operations",
	"core_objects",
	"core_records",
	"core_previews",
	"message_file_versions",
	"semantic_image_publications",
];

/// execution 0148: the Working Area key becomes (home Node, Thread, Agent,
/// owner). Areas that differed only by their reply thread or Root Task leave
/// one per key (see [`survivor`]). Every row that named a merged Area names
/// the survivor; a merged Area's Runs queue after the survivor's own, in
/// their order. The merged Areas' manifests, holds and Live previews cascade,
/// so files a person kept in a merged Area are dropped.
pub(crate) fn duplicate_areas() -> String {
	let area = |table: &str| format!("{table}.area_id");
	let mut statements = Vec::new();
	// A merged Run takes the survivor's generation and the next free places
	// of its queue, after the survivor's own Runs.
	let survivor_area = survivor_of(&area("core_runs"));
	let survivor_column = |column: &str| {
		Expr::cust(format!(
			"(SELECT s.{column} FROM core_areas AS s WHERE s.id = (SELECT {} FROM core_areas AS a WHERE a.id = core_runs.area_id))",
			survivor("a")
		))
	};
	let earlier_merged = format!(
		"(SELECT count(*) FROM core_runs AS o JOIN core_areas AS oa ON oa.id = o.area_id WHERE oa.id <> {survivor} AND {survivor} = (SELECT {mine} FROM core_areas AS a WHERE a.id = core_runs.area_id) AND (o.area_id, o.sequence) < (core_runs.area_id, core_runs.sequence))",
		survivor = survivor("oa"),
		mine = survivor("a"),
	);
	statements.push(
		Query::update()
			.table(Alias::new("core_runs"))
			.value_expr(Alias::new("area_id"), survivor_area)
			.value_expr(Alias::new("generation"), survivor_column("generation"))
			.value_expr(
				Alias::new("sequence"),
				survivor_column("next_sequence").add(Expr::cust(earlier_merged)),
			)
			.and_where(merged(&area("core_runs")))
			.to_string(PostgresQueryBuilder),
	);
	let past_last = || {
		Expr::cust(
			"(SELECT max(q.sequence) + 1 FROM core_runs AS q WHERE q.area_id = core_areas.id)",
		)
	};
	statements.push(
		Query::update()
			.table(Alias::new("core_areas"))
			.value_expr(Alias::new("next_sequence"), past_last())
			.and_where(past_last().gt(Expr::col(Alias::new("next_sequence"))))
			.to_string(PostgresQueryBuilder),
	);
	for table in AREA_HOLDERS {
		statements.push(
			Query::update()
				.table(Alias::new(table))
				.value_expr(Alias::new("area_id"), survivor_of(&area(table)))
				.and_where(merged(&area(table)))
				.to_string(PostgresQueryBuilder),
		);
	}
	statements.push(
		Query::delete()
			.from_table(Alias::new("core_areas"))
			.and_where(Expr::cust(format!(
				"core_areas.id <> {}",
				survivor("core_areas")
			)))
			.to_string(PostgresQueryBuilder),
	);
	format!("{DEPLOYMENT_SCOPE}{};\n", statements.join(";\n"))
}

/// identity 0110: Thread Approval Modes named reply threads, which are gone.
pub(crate) fn reply_thread_modes() -> String {
	format!(
		"{DEPLOYMENT_SCOPE}{};\n",
		Query::delete()
			.from_table(Alias::new("identity_thread_approval_modes"))
			.to_string(PostgresQueryBuilder)
	)
}

#[cfg(test)]
mod tests {
	#[test]
	fn the_data_statements_name_the_columns_before_their_rename() {
		let areas = super::duplicate_areas();
		assert!(
			areas.contains("s.workspace_id = core_areas.workspace_id"),
			"{areas}"
		);
		assert!(areas.contains("q.area_id = core_areas.id"), "{areas}");
		assert!(
			super::reply_thread_modes().contains("DELETE FROM \"identity_thread_approval_modes\"")
		);
	}
}
