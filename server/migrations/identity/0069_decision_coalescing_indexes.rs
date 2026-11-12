// reinhardt-migration-source: 1
// One coalesced allow per Run or Remote Grant, subject, action, resource and policy
// revision (ADR 0116). A race between two checks ends at these indexes.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0069_decision_coalescing_indexes", "identity")
		.add_dependency("identity", "0068_decision_coalescing")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateNamedIndex {
			table: "authorization_decisions".to_owned(),
			name: "authorization_decisions_coalesced_run".to_owned(),
			columns: vec![
				"tenant".to_owned(),
				"run_id".to_owned(),
				"revision".to_owned(),
				"subject".to_owned(),
				"action".to_owned(),
				"resource_kind".to_owned(),
				"resource_id".to_owned(),
			],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(coalesced AND run_id IS NOT NULL)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "authorization_decisions".to_owned(),
			name: "authorization_decisions_coalesced_remote_grant".to_owned(),
			columns: vec![
				"tenant".to_owned(),
				"remote_grant_id".to_owned(),
				"revision".to_owned(),
				"subject".to_owned(),
				"action".to_owned(),
				"resource_kind".to_owned(),
				"resource_id".to_owned(),
			],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(coalesced AND remote_grant_id IS NOT NULL)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
