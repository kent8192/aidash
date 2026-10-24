// reinhardt-migration-source: 1
// What the billing tables keep true. Enterprise references Community, never
// the reverse (ADR 0032). A Tenant's root record is never deleted, so a
// reference to it outlives the Tenant's purge; identities are never deleted.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0002_constraints", "billing")
		.add_dependency("billing", "0001_tables")
		.add_dependency("identity", "0031_tenant_deletions_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_accounts".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_accounts_owner_identity_fkey".to_owned(),
				columns: vec!["owner_identity".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_subscriptions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_subscriptions_account_id_fkey".to_owned(),
				columns: vec!["account_id".to_owned()],
				referenced_table: "billing_accounts".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_subscriptions".to_owned(),
			constraint: Constraint::Check {
				name: "billing_subscriptions_status".to_owned(),
				expression: "status IN ('active','past_due','suspended','canceled') AND (grace_ends_at IS NULL OR status = 'past_due')".to_owned(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachments".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_attachments_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachments".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_attachments_account_id_fkey".to_owned(),
				columns: vec!["account_id".to_owned()],
				referenced_table: "billing_accounts".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachments".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_attachments_consented_by_administrator_fkey".to_owned(),
				columns: vec!["consented_by_administrator".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachments".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_attachments_consented_by_owner_fkey".to_owned(),
				columns: vec!["consented_by_owner".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "billing_attachments".into(),
			name: "billing_attachments_account".into(),
			columns: vec!["account_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachment_consents".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_attachment_consents_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachment_consents".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_attachment_consents_account_id_fkey".to_owned(),
				columns: vec!["account_id".to_owned()],
				referenced_table: "billing_accounts".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachment_consents".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_attachment_consents_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_attachment_consents".to_owned(),
			constraint: Constraint::Check {
				name: "billing_attachment_consents_party".to_owned(),
				expression: "party IN ('tenant_administrator','account_owner') AND expires_at > created_at".to_owned(),
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "billing_attachment_consents".into(),
			name: "billing_attachment_consents_party_key".into(),
			columns: vec!["tenant".into(), "account_id".into(), "party".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_pod_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_pod_usage_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_pod_usage".to_owned(),
			constraint: Constraint::Check {
				name: "billing_pod_usage_span".to_owned(),
				expression: "ended_at >= started_at AND millis > 0".to_owned(),
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "billing_pod_usage".into(),
			name: "billing_pod_usage_tenant_ended".into(),
			columns: vec!["tenant".into(), "ended_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_storage_samples".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "billing_storage_samples_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_storage_samples".to_owned(),
			constraint: Constraint::Check {
				name: "billing_storage_samples_bytes".to_owned(),
				expression: "bytes >= 0".to_owned(),
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "billing_storage_samples".into(),
			name: "billing_storage_samples_day".into(),
			columns: vec!["tenant".into(), "sampled_on".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
