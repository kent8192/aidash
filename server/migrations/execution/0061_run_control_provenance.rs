// reinhardt-migration-source: 1
// A Run's control provenance (#485, ADR 0101). `control_revision` changes only
// when control changes, so a Safety Stop's expected revision is not made stale
// by worker progress (`revision` moves with every step). `control_source` is the
// kind of actor that last changed control: the Tenant, a Deployment
// Administrator's Safety Stop, or the system. Existing Runs start at revision 0
// with the Tenant as the source.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0061_run_control_provenance", "execution")
		.add_dependency("execution", "0060_plugin_research_merge")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "runs".into(),
			column: ColumnDefinition::new("control_revision", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "runs".into(),
			column: ColumnDefinition::new("control_source", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("'tenant'".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".into(),
			constraint: Constraint::Check {
				name: "runs_control_provenance".into(),
				expression: "control_revision >= 0 AND control_source IN ('tenant', 'deployment_administrator', 'system')".into(),
			},
		})
}
