// reinhardt-migration-source: 1
// Existing notices are already handled. New claims keep a lease and the
// triggering spend until SMTP accepts delivery or the notice is skipped.
use reinhardt::db::migrations::{FieldType, prelude::*};

pub(super) fn migration() -> Migration {
	Migration::new("0113_budget_notice_delivery", "identity")
		.add_dependency("identity", "0112_budget_lineage")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AlterColumn {
			table: "budget_notices".into(),
			column: "sent_at".into(),
			old_definition: Some(
				ColumnDefinition::new("sent_at", FieldType::TimestampTz).with_not_null(true),
			),
			new_definition: ColumnDefinition::new("sent_at", FieldType::TimestampTz),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "budget_notices".into(),
			column: ColumnDefinition::new("claim_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "budget_notices".into(),
			column: ColumnDefinition::new("claim_until", FieldType::TimestampTz),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "budget_notices".into(),
			column: ColumnDefinition::new("spent_nanocredits", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "budget_notices".into(),
			column: ColumnDefinition::new("skipped", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "budget_notices".into(),
			constraint: Constraint::Check {
				name: "budget_notices_delivery_check".into(),
				expression: "(sent_at IS NOT NULL OR skipped OR spent_nanocredits IS NOT NULL) AND (claim_id IS NULL) = (claim_until IS NULL) AND ((sent_at IS NULL AND NOT skipped) OR claim_id IS NULL) AND NOT (sent_at IS NOT NULL AND skipped)".into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: "SELECT 1".into(),
			reverse_sql: Some(include_str!("sql/backward/0113_budget_notice_delivery.sql").into()),
		})
}
