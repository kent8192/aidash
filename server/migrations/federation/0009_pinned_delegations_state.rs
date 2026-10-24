// reinhardt-migration-source: 1
//! Model state for the retained, optional receiver closure.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0009_pinned_delegations_state", "federation")
		.state_only(true)
		.add_dependency("federation", "0008_pinned_delegations")
		.add_operation(Operation::AddColumn {
			table: "delegations".into(),
			column: ColumnDefinition::new("binding_snapshot", FieldType::Jsonb),
			mysql_options: None,
		})
}
