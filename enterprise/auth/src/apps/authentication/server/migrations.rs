// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Independent migration set, including the framework's durable OAuth/OIDC stores.
use reinhardt::auth::{oauth2_server::PostgresOAuthStore, oidc_op::PostgresOidcStore};
use reinhardt::db::migrations::{FieldType, prelude::*};

pub fn migrations() -> Vec<Migration> {
	let table = |name: &str, columns| Operation::CreateTable {
		name: name.into(),
		columns,
		constraints: vec![],
		without_rowid: None,
		interleave_in_parent: None,
		partition: None,
	};
	let id = || {
		ColumnDefinition::new("id", FieldType::Uuid)
			.with_not_null(true)
			.with_primary_key(true)
	};
	let document = || ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true);
	let text = |name| ColumnDefinition::new(name, FieldType::Text).with_not_null(true);
	let migration = Migration::new("0001_accounts", "authentication")
		.add_operation(table("auth_accounts", vec![id(), document()]))
		.add_operation(Operation::CreateTable {
			name: "auth_identities".into(),
			columns: vec![id(), text("issuer"), text("subject"), document()],
			constraints: vec![Constraint::Unique {
				name: "auth_identity_source".into(),
				columns: vec!["issuer".into(), "subject".into()],
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(table(
			"auth_sessions",
			vec![
				id(),
				text("session_kind"),
				ColumnDefinition::new("expires_at", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("account_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true),
				document(),
			],
		))
		.add_operation(table(
			"auth_source_observations",
			vec![
				ColumnDefinition::new("identity_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("valid_since", FieldType::TimestampTz),
			],
		))
		.add_operation(table(
			"auth_proofs",
			vec![
				id(),
				ColumnDefinition::new("expires_at", FieldType::BigInteger).with_not_null(true),
				document(),
			],
		))
		.add_operation(table(
			"auth_flows",
			vec![
				id(),
				ColumnDefinition::new("expires_at", FieldType::BigInteger),
				text("browser_digest"),
				document(),
			],
		))
		.add_operation(table(
			"auth_token_bindings",
			vec![
				text("digest").with_primary_key(true),
				ColumnDefinition::new("expires_at", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("session_id", FieldType::Uuid).with_not_null(true),
				text("kind"),
			],
		))
		.add_operation(table(
			"auth_refresh_responses",
			vec![
				text("digest").with_primary_key(true),
				ColumnDefinition::new("expires_at", FieldType::BigInteger).with_not_null(true),
				document(),
			],
		))
		.add_operation(table(
			"auth_rate_limits",
			vec![
				text("bucket").with_primary_key(true),
				ColumnDefinition::new("minute", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("attempts", FieldType::BigInteger).with_not_null(true),
			],
		));
	let profiles = Migration::new("0002_ephemeral_source_profiles", "authentication")
		.add_dependency("authentication", "0001_accounts")
		.add_operation(table(
			"auth_ephemeral_profiles",
			vec![
				id(),
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true),
				text("kind"),
				ColumnDefinition::new("auth_time", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("verified_email", FieldType::Text),
				ColumnDefinition::new("display_name", FieldType::Text),
			],
		));
	[
		vec![migration, profiles],
		PostgresOAuthStore::migrations(),
		vec![PostgresOidcStore::migration()],
	]
	.concat()
}
