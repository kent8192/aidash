// reinhardt-migration-source: 1
// A remote read records whether its Node belongs to the reader's Tenant (#769). Rows written
// before this are treated as another Tenant's, which keeps asking the Node (the stricter check).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0070_remote_reads_same_tenant", "identity")
		.add_dependency("identity", "0069_decision_coalescing_indexes")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_run_remote_reads".into(),
			column: ColumnDefinition::new("same_tenant", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
}
