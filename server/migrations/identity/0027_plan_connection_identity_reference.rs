// reinhardt-migration-source: 1
// A Plan Connection names the External Identity that connected it (#315), as
// every other `identity_id` column does. Identities are never deleted, so the
// reference does not cascade.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0027_plan_connection_identity_reference", "identity")
		.add_dependency("identity", "0026_plan_connection_identity")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "plan_connections".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "plan_connections_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
}
