// reinhardt-migration-source: 1
// Current trust may outlive the finite Node catalog while immutable atomic
// journals still authorize exact recovery. Logical NodeRegistration is scalar;
// this changes only physical ownership, preserving Tenant FK/RLS and authority.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0019_node_registration_recovery_ownership", "federation")
        // Rechain to reviewed cancellation's 0018 before final integration.
        .add_dependency("federation", "0016_receiving_keysets_state")
        .database_only(true).atomic(true)
        .add_operation(Operation::DropConstraintDefinition {
            table: "node_registrations".into(),
            constraint: Constraint::ForeignKey {
                name: "node_registrations_node_fkey".into(),
                columns: vec!["tenant".into(), "node".into()],
                referenced_table: "nodes".into(),
                referenced_columns: vec!["tenant".into(), "name".into()],
                on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None,
            },
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "node_registrations".into(),
            constraint: Constraint::ForeignKey {
                name: "node_registrations_node_fkey".into(),
                columns: vec!["node".into(), "tenant".into()],
                referenced_table: "node_names".into(),
                referenced_columns: vec!["name".into(), "tenant".into()],
                on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None,
            },
        })
}
