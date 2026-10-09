// reinhardt-migration-source: 1
//! GCIP External Identity keys, display attributes and session authentication time.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0012_gcip_sign_in", "identity")
		.add_dependency("identity", "0011_home_human_requests_state")
		.add_operation(Operation::AddColumn {
			table: "dashboard_identities".into(),
			column: ColumnDefinition::new("gcip_tenant", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("''".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_identities".into(),
			column: ColumnDefinition::new("valid_since", FieldType::TimestampTz)
				.with_not_null(false)
				.with_default(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_identities".into(),
			column: ColumnDefinition::new("verified_email", FieldType::Text)
				.with_not_null(false)
				.with_default(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_identities".into(),
			column: ColumnDefinition::new("display_name", FieldType::Text)
				.with_not_null(false)
				.with_default(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_sessions".into(),
			column: ColumnDefinition::new("auth_time", FieldType::TimestampTz)
				.with_not_null(false)
				.with_default(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_login_transactions".into(),
			column: ColumnDefinition::new("started_at", FieldType::TimestampTz)
				.with_not_null(false)
				.with_default(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_login_transactions".into(),
			column: ColumnDefinition::new("gcip_tenant", FieldType::Text)
				.with_not_null(false)
				.with_default(None),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "dashboard_identities".into(),
			constraint: Constraint::Unique {
				name: "dashboard_identity_key".into(),
				columns: vec!["issuer".into(), "subject".into()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_identities".into(),
			constraint: Constraint::Unique {
				name: "dashboard_identity_key".into(),
				columns: vec!["issuer".into(), "gcip_tenant".into(), "subject".into()],
			},
		})
}
