// reinhardt-migration-source: 1
// Budgets (#1012, ADR 0143): optional money limits on the Tenant, a Node, a
// Duty or a person; the Soft Limit mail sent per window; and the spend ledger,
// one row per day, Node, root person and Duty. The ledger is kept apart from
// the usage records, which go with their Runs: deleting a Duty, a Thread or a
// Node must not give money back to the Budgets that covered it.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0100_budgets", "identity")
		.add_dependency("identity", "0099_task_origin_federated")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "budgets".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("period", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("soft_limit_nanocredits", FieldType::BigInteger),
				ColumnDefinition::new("hard_limit_nanocredits", FieldType::BigInteger),
				ColumnDefinition::new("on_hard_limit", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("recipients", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("created_by", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "budgets_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Unique {
					name: "budgets_tenant_id_key".into(),
					columns: vec!["tenant".into(), "id".into()],
				},
				Constraint::Unique {
					name: "budgets_scope_key".into(),
					columns: vec!["tenant".into(), "scope_kind".into(), "scope_key".into()],
				},
				Constraint::Check {
					name: "budgets_valid".into(),
					expression: "scope_kind IN ('tenant','node','duty','person')  AND period IN ('monthly','total')  AND on_hard_limit IN ('stop','sleep')  AND revision > 0  AND (soft_limit_nanocredits IS NOT NULL OR hard_limit_nanocredits IS NOT NULL)  AND (soft_limit_nanocredits IS NULL OR soft_limit_nanocredits > 0)  AND (hard_limit_nanocredits IS NULL OR hard_limit_nanocredits > 0)  AND (soft_limit_nanocredits IS NULL OR hard_limit_nanocredits IS NULL  OR soft_limit_nanocredits < hard_limit_nanocredits)  AND cardinality(recipients) <= 20"
						.into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "budget_notices".into(),
			columns: vec![
				ColumnDefinition::new("budget_id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("window_start", FieldType::TimestampTz)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("sent_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "budget_notices_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "budget_notices_budget_fkey".into(),
					columns: vec!["tenant".into(), "budget_id".into()],
					referenced_table: "budgets".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "budget_spend".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("day", FieldType::TimestampTz)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("root_subject", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				// The nil UUID when the spend belongs to no Duty.
				ColumnDefinition::new("duty_id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("spent_nanocredits", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("unpriced_calls", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "budget_spend_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "budget_spend_valid".into(),
					expression: "spent_nanocredits >= 0 AND unpriced_calls >= 0  AND day = date_trunc('day', day, 'UTC')"
						.into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
