//! Unit tests for services::peer::graph::catalog.
use super::*;

fn options(mode: &str, kinds: &[&str]) -> GraphOptions {
	GraphOptions {
		scope_workspace: None,
		depth: 1,
		mode: mode.into(),
		kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
		relations: vec![],
		hours: 0,
		limit: 80,
		cursor: None,
		target_tenant: None,
	}
}

#[rstest::rstest]
fn kind_filter_precedes_pagination() {
	let (sql, kinds) = query("acme", &options("mesh", &["agent", "run"]), 4096).unwrap();
	assert_eq!(kinds, vec!["agent"]);
	let predicate = sql.find("r.metadata->>'kind' IN ('agent')").unwrap();
	assert!(predicate < sql.find("LIMIT 64").unwrap());
	assert!(sql.contains("OFFSET 4096"));
}

#[rstest::rstest]
fn perspective_filters_catalog_kinds() {
	let (_, kinds) = query(
		"acme",
		&options("execution", &["agent", "tool", "model", "run"]),
		0,
	)
	.unwrap();
	assert_eq!(kinds, vec!["agent"]);
	let (_, kinds) = query("acme", &options("topology", &["agent", "tool", "model"]), 0).unwrap();
	assert_eq!(kinds, vec!["agent", "tool"]);
}

#[rstest::rstest]
fn irrelevant_kinds_skip_the_catalog_query() {
	assert!(query("acme", &options("mesh", &["workspace", "goal", "run"]), 0).is_none());
	assert!(query("acme", &options("execution", &["tool", "model"]), 0).is_none());
	assert!(query("acme", &options("mesh", &[]), 0).is_none());
}
