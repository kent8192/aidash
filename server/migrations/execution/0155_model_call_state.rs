// reinhardt-migration-source: 1
//! Register the Usage Record's optional Run relation and Model Call id.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0155_model_call_state", "execution")
		.add_dependency("execution", "0154_model_calls")
		.state_only(true)
		.add_operation(Operation::AddColumn {
			table: "inference_usage".into(),
			column: ColumnDefinition::new("model_call_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "inference_usage".into(),
			column: "run_id".into(),
			old_definition: Some(
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
			),
			new_definition: ColumnDefinition::new("run_id", FieldType::Uuid),
			mysql_options: None,
		})
}
