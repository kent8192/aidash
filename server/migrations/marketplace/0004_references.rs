// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0004_references", "marketplace")
		.database_only(true)
		.add_dependency("knowledge", "0004_references")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_revisions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "marketplace_revisions_entry_id_entry_version_fkey".to_owned(),
				columns: vec!["entry_id".to_owned(), "entry_version".to_owned()],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_revisions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "marketplace_revisions_installation_fkey".to_owned(),
				columns: vec!["installation".to_owned()],
				referenced_table: "marketplace_installations".to_owned(),
				referenced_columns: vec!["key".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
