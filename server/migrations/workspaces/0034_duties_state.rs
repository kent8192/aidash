// reinhardt-migration-source: 1
// ORM state of the removed Tenant Duty limits; 0033 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0034_duties_state", "workspaces")
		.add_dependency("workspaces", "0033_duty_checks")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "schedule_limits".into(),
		})
}
