// reinhardt-migration-source: 1
// Catalog Approval Requests (ADR 0096): one Tenant subject asks an approver to
// review one exact root on one Node, pinning the closure digest they saw. A
// request grants no authority. One open request exists per requester, root and
// Node; every other state is final.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0042_catalog_approval_requests", "identity")
		.database_only(true)
		.add_dependency("identity", "0041_catalog_restriction")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "catalog_approval_requests".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("root_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("root_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("requester", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("closure_digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("decline_reason", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("decided_by", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "catalog_approval_requests_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "catalog_approval_requests_state".into(),
					expression: "state IN ('open','fulfilled','declined','stale','withdrawn')".into(),
				},
				Constraint::Check {
					name: "catalog_approval_requests_reason".into(),
					expression: "length(btrim(reason)) BETWEEN 1 AND 2000".into(),
				},
				Constraint::Check {
					name: "catalog_approval_requests_decline".into(),
					expression: "((state = 'declined') = (decline_reason IS NOT NULL)) AND (decline_reason IS NULL OR length(btrim(decline_reason)) BETWEEN 1 AND 2000)".into(),
				},
				Constraint::Check {
					name: "catalog_approval_requests_revision".into(),
					expression: "(revision > 0)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// One open request per (tenant, requester, root, Node); a final request
		// frees the tuple for a later request.
		.add_operation(Operation::CreateNamedIndex {
			table: "catalog_approval_requests".into(),
			name: "catalog_approval_requests_open".into(),
			columns: vec![
				"tenant".into(),
				"requester".into(),
				"root_id".into(),
				"root_version".into(),
				"node".into(),
			],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("state = 'open'".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
