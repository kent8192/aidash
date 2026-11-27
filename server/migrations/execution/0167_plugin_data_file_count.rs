// reinhardt-migration-source: 1
// Retain the storage bounds on raw content and the packed value.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0167_plugin_data_file_count", "execution")
		.add_dependency("execution", "0166_scope_connection_debits")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "agent_plugin_data".into(),
			constraint: Constraint::Check {
				name: "agent_plugin_data_bounds".into(),
				expression: "size BETWEEN 0 AND 67108864 AND files BETWEEN 0 AND 4096 \
					AND octet_length(data) <= 75497472 AND sha256 ~ '^[0-9a-f]{64}$'"
					.into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_plugin_data".into(),
			constraint: Constraint::Check {
				name: "agent_plugin_data_bounds".into(),
				expression: "size BETWEEN 0 AND 67108864 AND files >= 0 \
					AND octet_length(data) <= 75497472 AND sha256 ~ '^[0-9a-f]{64}$'"
					.into(),
			},
		})
}
