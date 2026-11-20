// reinhardt-migration-source: 1
// A Duty installed from a Marketplace Duty template records the template
// version it came from (#1011 D4), so a later version can be offered to its
// Requester as an update, and the install request that created it, so a
// retried install returns it. The column is in the ORM model; 0061 adds the
// database-only check of its shape.
use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0060_duty_template", "workspaces")
		.add_dependency("workspaces", "0050_threads_state")
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("template", FieldType::Jsonb)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.atomic(true)
}
