// reinhardt-migration-source: 1
// Read Coalescing (ADR 0116): an Authorization Decision names the Run or Remote
// Grant it was made for. Existing rows name neither and stay as they are; Aidash
// is undeployed, so nothing is backfilled.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0068_decision_coalescing", "identity")
		.add_dependency("identity", "0067_scratch_member_policy")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_decisions".into(),
			column: ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_decisions".into(),
			column: ColumnDefinition::new("remote_grant_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_decisions".into(),
			column: ColumnDefinition::new("coalesced", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
}
