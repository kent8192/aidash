// reinhardt-migration-source: 1
// Model state for Purged snapshots and Evidence Revocation records. The
// physical purge check and the retention indexes stay database-only.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0028_research_retention_state", "execution")
		.add_operation(Operation::CreateTable {
			name: "research_revocations".to_string(),
			columns: vec![
				ColumnDefinition::new("reason", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("removed", FieldType::Jsonb)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("revoked_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("revoked_by", FieldType::Jsonb)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("run_id", FieldType::Uuid)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(true)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(false)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "research_snapshots".to_string(),
			column: ColumnDefinition::new("purge_reason", FieldType::Text)
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
			table: "research_snapshots".to_string(),
			column: ColumnDefinition::new("purged_at", FieldType::TimestampTz)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.add_dependency("execution", "0027_research_retention")
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
