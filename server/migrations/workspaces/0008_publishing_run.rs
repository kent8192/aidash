// reinhardt-migration-source: 1
// The Run that published a message or Artifact on this Node. Source citations
// in that text resolve only against this Run's Research evidence. It has no
// foreign key: it is kept after the Run is deleted, so such a citation reads as
// unavailable rather than as text nobody published.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0008_publishing_run", "workspaces")
		.add_operation(Operation::AddColumn {
			table: "messages".to_string(),
			column: ColumnDefinition::new("run_id", FieldType::Uuid)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "artifacts".to_string(),
			column: ColumnDefinition::new("run_id", FieldType::Uuid)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.add_dependency("workspaces", "0007_model_state")
		.atomic(true)
		.with_initial(None)
		.state_only(false)
		.database_only(false)
}
