// reinhardt-migration-source: 1
// A Run's control provenance (#485, ADR 0101). `control_revision` changes only
// when control changes, so a Safety Stop's expected revision is not made stale
// by worker progress (`revision` moves with every step). `control_source` is the
// kind of actor that last changed control: the Tenant, a Deployment
// Administrator's Safety Stop, or the system. Existing Runs start at revision 0
// with the Tenant as the source. The model declares no default for the source,
// so the logical state drops it as `execution/0050_usage_evidence` does; 0076
// keeps the physical default for writers that name no source.
use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0075_run_control_provenance", "execution")
		.add_dependency("execution", "0074_federated_preview_authority")
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
		.add_operation(Operation::AlterColumn {
			table: "runs".into(),
			column: "control_source".into(),
			old_definition: Some(
				ColumnDefinition::new("control_source", FieldType::Text)
					.with_not_null(true)
					.with_default(Some("'tenant'".into())),
			),
			new_definition: ColumnDefinition::new("control_source", FieldType::Text)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".into(),
			constraint: Constraint::EnumDomain {
				name: "runs_control_source_model_enum_check".into(),
				column: "control_source".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("deployment_administrator".into()),
						ModelEnumValue::String("system".into()),
						ModelEnumValue::String("tenant".into()),
					],
				},
			},
		})
}
