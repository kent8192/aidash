// reinhardt-migration-source: 1
// Tenant Data Protection Level and its history (#542).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0041_tenant_data_protection", "identity")
		.add_dependency("identity", "0040_common_auth_origin_binding")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_data_protection".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("level", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true)],
			constraints: vec![
				Constraint::ForeignKey {
			name: "tenant_data_protection_tenant_root_fkey".into(),
			columns: vec![
				"tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Check {
			name: "tenant_data_protection_valid".into(), expression: "level IN ('standard', 'no_training', 'zero_data_retention') AND (level <> 'zero_data_retention' OR model_call_zdr)".into() }
			],
			without_rowid: None, interleave_in_parent: None, partition: None })
		.add_operation(Operation::CreateTable {
			name: "tenant_data_protection_history".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("level", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("changed_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("changed_by", FieldType::Text).with_not_null(true)],
			constraints: vec![
				Constraint::ForeignKey {
			name: "tenant_data_protection_history_tenant_root_fkey".into(),
			columns: vec![
				"tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Check {
			name: "tenant_data_protection_history_valid".into(), expression: "level IN ('standard', 'no_training', 'zero_data_retention') AND (level <> 'zero_data_retention' OR model_call_zdr)".into() }
			],
			without_rowid: None, interleave_in_parent: None, partition: None })
}
