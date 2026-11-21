// reinhardt-migration-source: 1
// The template a Duty was installed from (#1011 D4) is an object naming the
// Marketplace version's key, Node, publisher, package and version. The check
// is database-only, as the Duty's other checks are.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0061_duty_template_check", "workspaces")
		.add_dependency("workspaces", "0060_duty_template")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "duties_template_check".into(),
				expression: "template IS NULL OR (jsonb_typeof(template) = 'object' AND jsonb_typeof(template->'key') = 'string' AND jsonb_typeof(template->'repository') = 'string' AND jsonb_typeof(template->'owner_tenant') = 'string' AND jsonb_typeof(template->'package_id') = 'string' AND jsonb_typeof(template->'version') = 'string')".into(),
			},
		})
}
