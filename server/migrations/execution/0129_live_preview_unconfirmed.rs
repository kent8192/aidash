// reinhardt-migration-source: 1
// Since when a Live preview's Read Recheck could not be confirmed; while set,
// the Preview gateway withholds its responses (#906, ADR 0129).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0129_live_preview_unconfirmed", "execution")
		.add_dependency("execution", "0128_remove_generated_summary_state")
		.add_operation(Operation::AddColumn {
			table: "core_live_previews".into(),
			column: ColumnDefinition::new("unconfirmed_since", FieldType::TimestampTz),
			mysql_options: None,
		})
		.atomic(true)
}
