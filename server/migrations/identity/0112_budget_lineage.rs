// reinhardt-migration-source: 1
// Nested Budgets (#1012 decision 33, ADR 0143): what an Agent creates is
// carved out of its creator's Budget. A Duty a Duty's Run creates, and a Node
// a Run on a Node creates, records every Duty or Node above it, written once
// when it is created. A Duty or Node Budget then covers, and counts the spend
// of, everything below it. The lineage is Tenant-owned and outlives the Duties
// and Nodes it names, like the spend ledger, so deleting one gives no money
// back to the Budgets above it. Its row-level policy is a SQL asset because
// Reinhardt migrations cannot express policies (kent8192/reinhardt-web#6770).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0112_budget_lineage", "identity")
		.add_dependency("identity", "0111_thread_member_policy")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "budget_lineage".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				// `duty` (Duty ids) or `node` (Node names).
				ColumnDefinition::new("kind", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("child", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("ancestor", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "budget_lineage_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "budget_lineage_valid".into(),
					expression: "kind IN ('duty','node') AND child <> ancestor".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// A Duty or Node Budget sums the spend of everything below it.
		.add_operation(Operation::CreateNamedIndex {
			table: "budget_lineage".into(),
			name: "budget_lineage_ancestor".into(),
			columns: vec!["tenant".into(), "kind".into(), "ancestor".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0112_budget_lineage.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0112_budget_lineage.sql").to_owned()),
		})
}
