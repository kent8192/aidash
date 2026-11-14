// reinhardt-migration-source: 1
// Reinhardt cannot express PostgreSQL row policies or statement triggers.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0053_memory_journal_proofs", "knowledge")
		.add_dependency("knowledge", "0052_memory_sharing_scope")
		.add_dependency("execution", "0016_context_journal")
		.add_operation(Operation::CreateTable {
			name: "memory_journal_proofs".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("from_seq", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("through_seq", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("research", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "memory_journal_proofs_tenant_fkey".into(), columns: vec!["tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::ForeignKey { name: "memory_journal_proofs_run_fkey".into(), columns: vec!["tenant".into(), "run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
				Constraint::Check { name: "memory_journal_proofs_valid".into(), expression: "from_seq > 0 AND through_seq >= from_seq AND jsonb_typeof(research) = 'array'".into() },
			], without_rowid: None, interleave_in_parent: None, partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0053_memory_journal_proofs_policy.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0053_memory_journal_proofs_policy.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER memory_journal_proofs_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_journal_proofs FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
			reverse_sql: Some("DROP TRIGGER memory_journal_proofs_atomic ON memory_journal_proofs;".into()),
		}).atomic(true)
}
