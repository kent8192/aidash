// reinhardt-migration-source: 1
// Model state for a Tenant's own Retention Rules and their history. The
// physical period checks and the history index stay database-only.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0025_tenant_retention_rules_state", "identity")
		.add_operation(Operation::CreateTable {
			name: "tenant_retention_rule_history".to_string(),
			columns: vec![
				ColumnDefinition::new("changed_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("changed_by", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(true)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("snapshot_after_run_seconds", FieldType::Integer)
					.with_not_null(false)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("snapshot_max_age_seconds", FieldType::Integer)
					.with_not_null(false)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
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
		.add_operation(Operation::CreateTable {
			name: "tenant_retention_rules".to_string(),
			columns: vec![
				ColumnDefinition::new("snapshot_after_run_seconds", FieldType::Integer)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("snapshot_max_age_seconds", FieldType::Integer)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(true)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("updated_by", FieldType::Text)
					.with_not_null(true)
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
		.add_dependency("identity", "0024_tenant_retention_rules")
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
