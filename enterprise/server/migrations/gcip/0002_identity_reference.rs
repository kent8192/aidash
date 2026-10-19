// reinhardt-migration-source: 1
// Enterprise references Community, never the reverse (ADR 0032). Identities
// are never deleted, so the reference does not cascade.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0002_identity_reference", "gcip")
		.add_dependency("gcip", "0001_admissions")
		.add_dependency("identity", "0003_keys_indexes")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "gcip_admissions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "gcip_admissions_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
}
