// reinhardt-migration-source: 1
// How each GCIP External Identity was admitted (#252, #240). Deployment-wide.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0001_admissions", "gcip")
		.add_dependency("identity", "0002_tables")
		.add_operation(Operation::CreateTable {
			name: "gcip_admissions".to_string(),
			columns: vec![
				ColumnDefinition::new("admitted_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("gcip_tenant", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("identity_id", FieldType::Uuid)
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
		.atomic(true)
		.with_initial(Some(true))
		.state_only(false)
		.database_only(false)
}
