use super::*;

fn reference(child: &str, parent: &str) -> Reference {
	through(child, parent, "id")
}

fn through(child: &str, parent: &str, column: &str) -> Reference {
	Reference {
		child: child.into(),
		parent: parent.into(),
		columns: vec!["tenant".into(), column.into()],
		parent_columns: vec!["tenant".into(), "id".into()],
		updates_child: false,
		cascades: false,
	}
}

#[test]
fn children_are_deleted_before_their_parents() {
	let references = [
		reference("messages", "tasks"),
		reference("tasks", "workspaces"),
		reference("messages", "workspaces"),
	];

	let order = deletion_order(["workspaces", "messages", "tasks"], &references, &[]).unwrap();

	assert_eq!(order, ["messages", "tasks", "workspaces"]);
}

#[test]
fn a_table_referencing_itself_is_deleted_in_one_statement() {
	let order = deletion_order(["tasks"], &[reference("tasks", "tasks")], &[]).unwrap();

	assert_eq!(order, ["tasks"]);
}

#[test]
fn references_outside_the_set_do_not_order_it() {
	let references = [reference("runs", "tenants"), reference("audit", "runs")];

	let order = deletion_order(["runs"], &references, &[]).unwrap();

	assert_eq!(order, ["runs"]);
}

#[test]
fn tables_that_reference_each_other_in_a_cycle_are_refused() {
	let references = [reference("a", "b"), reference("b", "a")];

	assert!(deletion_order(["a", "b"], &references, &[]).is_err());
}

#[test]
fn the_order_does_not_depend_on_how_references_are_listed() {
	let forward = [reference("b", "a"), reference("c", "b")];
	let backward = [reference("c", "b"), reference("b", "a")];

	assert_eq!(
		deletion_order(["a", "b", "c"], &forward, &[]).unwrap(),
		deletion_order(["c", "a", "b"], &backward, &[]).unwrap()
	);
}

#[test]
fn an_unlinked_column_breaks_a_cycle() {
	let references = [
		through("runs", "human_requests", "pending_request"),
		through("human_requests", "runs", "run_id"),
	];

	let order = deletion_order(
		["runs", "human_requests"],
		&references,
		&[("runs", "pending_request")],
	)
	.unwrap();

	// Only human_requests.run_id still orders them: its rows go first.
	assert_eq!(order, ["human_requests", "runs"]);
}

#[test]
fn a_cascading_reference_does_not_order_the_tables() {
	let references = [
		Reference {
			cascades: true,
			..through("human_requests", "runs", "run_id")
		},
		through("runs", "human_requests", "pending_request"),
	];

	let order = deletion_order(["runs", "human_requests"], &references, &[]).unwrap();

	assert_eq!(order, ["runs", "human_requests"]);
}
