// reinhardt-migration-source: 1
// Tenant Deletions (ADR 0060): one Deployment-wide row per recorded deletion,
// so it outlives purge and is readable while the Tenant's scope refuses
// everyone. A Tenant has at most one deletion that is not restored; a purged
// deletion is never restored, so a purged Tenant stays deleted. The purge
// columns record how far a purge got and why it last failed (ADR 0061).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0030_tenant_deletions", "identity")
		.database_only(true)
		.add_dependency("identity", "0029_row_level_security")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_deletions".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("deleted_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("deleted_by_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("deleted_by_identity", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("purge_after", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("restored_at", FieldType::TimestampTz).with_not_null(false),
				ColumnDefinition::new("restored_by_kind", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("restored_by_identity", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("purged_at", FieldType::TimestampTz).with_not_null(false),
				// Purge progress (ADR 0061). `purge_step` is the step that runs next,
				// or the one that keeps failing; the failure columns describe it.
				ColumnDefinition::new("purge_step", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("purge_failures", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("purge_error", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("purge_failed_at", FieldType::TimestampTz).with_not_null(false),
				// The worker that owns the purge, and until when. Every destructive
				// step locks the row and checks it still holds this lease.
				ColumnDefinition::new("purge_lease", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("purge_leased_until", FieldType::TimestampTz).with_not_null(false),
				ColumnDefinition::new("audit_erased_at", FieldType::TimestampTz).with_not_null(false),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "tenant_deletions_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "tenant_deletions_deleted_by_identity_fkey".into(),
					columns: vec!["deleted_by_identity".into()],
					referenced_table: "dashboard_identities".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "tenant_deletions_restored_by_identity_fkey".into(),
					columns: vec!["restored_by_identity".into()],
					referenced_table: "dashboard_identities".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "tenant_deletions_state".into(),
					expression: "deleted_by_kind IN ('tenant_administrator','deployment_administrator') AND (deleted_by_kind <> 'tenant_administrator' OR deleted_by_identity IS NOT NULL) AND purge_after > deleted_at AND (restored_at IS NULL) = (restored_by_kind IS NULL) AND (restored_by_kind IS NULL OR restored_by_kind IN ('tenant_administrator','deployment_administrator')) AND (restored_by_kind IS DISTINCT FROM 'tenant_administrator' OR restored_by_identity IS NOT NULL) AND (restored_at IS NULL OR restored_at >= deleted_at) AND (restored_at IS NULL OR purged_at IS NULL) AND (purge_step IS NULL OR purge_step IN ('provider_credentials','capability_objects','rows','finish')) AND purge_failures >= 0 AND (purged_at IS NULL OR purge_step IS NULL) AND (purge_failures = 0 OR purge_error IS NOT NULL) AND (audit_erased_at IS NULL OR purged_at IS NOT NULL) AND (purge_lease IS NULL) = (purge_leased_until IS NULL)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// The deletion every boundary checks, at most one per Tenant.
		.add_operation(Operation::CreateNamedIndex {
			table: "tenant_deletions".into(),
			name: "tenant_deletions_current".into(),
			columns: vec!["tenant".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("restored_at IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// Deletions awaiting purge, oldest deadline first, for the purge sweep.
		.add_operation(Operation::CreateNamedIndex {
			table: "tenant_deletions".into(),
			name: "tenant_deletions_awaiting_purge".into(),
			columns: vec!["purge_after".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("restored_at IS NULL AND purged_at IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// Purged Tenants whose audit history is still kept, for the sweep that
		// deletes it once its period ends.
		.add_operation(Operation::CreateNamedIndex {
			table: "tenant_deletions".into(),
			name: "tenant_deletions_audit_kept".into(),
			columns: vec!["purged_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("purged_at IS NOT NULL AND audit_erased_at IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
