// reinhardt-migration-source: 1
// Binary renders as BINARY at the pinned Reinhardt revision (#6637).
// Physical bytea DDL precedes the equivalent Binary model snapshot.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0014_credential_store_schema", "identity").add_dependency("identity", "0013_provider_credential_constraints")
 .database_only(true).atomic(true)
.add_operation(Operation::CreateTable { name: "credential_store_resources".into(), columns: vec![
ColumnDefinition::new("resource", FieldType::Text).with_not_null(true).with_primary_key(true),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("next_version", FieldType::BigInteger).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_primary_key(false),
], constraints: vec![Constraint::Check { name: "credential_store_counter".into(), expression: "next_version > 0 AND length(tenant) BETWEEN 1 AND 256".into() }], without_rowid: None, interleave_in_parent: None, partition: None })
.add_operation(Operation::CreateTable { name: "credential_store_versions".into(), columns: vec![
ColumnDefinition::new("resource", FieldType::Text).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("version", FieldType::BigInteger).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("key_id", FieldType::Text).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("algorithm", FieldType::Text).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("nonce", FieldType::Bytea).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("ciphertext", FieldType::Bytea).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("state", FieldType::Text).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("disabled_at", FieldType::TimestampTz).with_not_null(false).with_primary_key(false),
], constraints: vec![Constraint::PrimaryKey { name: "credential_store_versions_pkey".into(), columns: vec!["resource".into(), "version".into()] },Constraint::ForeignKey { name: "credential_store_resource_fk".into(), columns: vec!["resource".into()], referenced_table: "credential_store_resources".into(), referenced_columns: vec!["resource".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::NoAction, deferrable: None },Constraint::Check { name: "credential_store_version_state".into(), expression: "version > 0 AND state IN ('enabled','disabled') AND (state <> 'disabled' OR disabled_at IS NOT NULL) AND octet_length(nonce) = 12".into() }], without_rowid: None, interleave_in_parent: None, partition: None })
.add_operation(Operation::CreateTable { name: "credential_store_keys".into(), columns: vec![
ColumnDefinition::new("key_id", FieldType::Text).with_not_null(true).with_primary_key(true),
ColumnDefinition::new("check_nonce", FieldType::Bytea).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("check_ciphertext", FieldType::Bytea).with_not_null(true).with_primary_key(false),
ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_primary_key(false),
], constraints: vec![], without_rowid: None, interleave_in_parent: None, partition: None })
}
