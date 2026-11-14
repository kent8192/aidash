//! Native descriptor admission and immutable system seeding on disposable PostgreSQL.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../execution/tests/support/tenant_roots.rs"]
mod tenant_roots;
use aidash_domain::{registry::bindings::QualifiedRef, tool::providers::ToolDescriptor};
use aidash_server::{
	apps::registry::{models::Definition, services::states::DefinitionKind},
	registry::{Entry, Registry, Search},
	store::Store,
};
use native_database::{DatabaseFixture, database};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;

async fn registry(database: &DatabaseFixture) -> Registry {
	let pool = database.connection.clone().into_postgres().unwrap();
	tenant_roots::tenant_root_in(&pool, "acme").await;
	let store = Store::from_pool(
		pool,
		aidash_domain::nodes::DeploymentId::parse("aidash://node").unwrap(),
	)
	.await
	.unwrap();
	Registry::new(store.pool.clone(), &store.deployment).unwrap()
}

#[rstest]
#[tokio::test]
async fn database_accepts_every_host_operation_and_its_exact_lifecycle(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	database.tenant("acme").await;
	let groups = aidash_application::registry::system::packages::declarations(
		&aidash_server::bootstrap::registry_validation(),
		"aidash://node",
	)
	.unwrap();
	let mut operations = groups
		.into_iter()
		.flat_map(|group| group.operations)
		.collect::<Vec<_>>();
	operations.sort_by_key(|entry| entry.config.get("lifecycle").is_some());
	let mut connection = database.lease.handle();
	for entry in operations {
		let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
		descriptor.validate().unwrap();
		let definition = Definition::build()
			.tenant("acme")
			.id(&entry.id)
			.version(&entry.version)
			.kind(DefinitionKind::Tool)
			.metadata(serde_json::to_value(&entry).unwrap().into())
			.finish();
		Definition::objects()
			.create_with_conn(&mut connection, &definition)
			.await
			.unwrap_or_else(|error| {
				panic!(
					"Host operation {} rejected: {error}; descriptor: {:?}",
					descriptor.operation, entry.config
				)
			});
		if descriptor.lifecycle.is_some() {
			for (index, mutation) in ["same-reference", "foreign-origin", "extra-key"]
				.into_iter()
				.enumerate()
			{
				let mut invalid = entry.clone();
				invalid.id = format!("invalid-{}-{index}", descriptor.operation);
				match mutation {
					"same-reference" => {
						invalid.config["lifecycle"]["cancel"] =
							invalid.config["lifecycle"]["poll"].clone()
					}
					"foreign-origin" => {
						invalid.config["lifecycle"]["poll"]["registry_deployment"] =
							json!("aidash://foreign")
					}
					_ => invalid.config["lifecycle"]["extra"] = json!(true),
				}
				let definition = Definition::build()
					.tenant("acme")
					.id(&invalid.id)
					.version(&invalid.version)
					.kind(DefinitionKind::Tool)
					.metadata(serde_json::to_value(&invalid).unwrap().into())
					.finish();
				let error = Definition::objects()
					.create_with_conn(&mut connection, &definition)
					.await
					.unwrap_err();
				assert!(
					error.to_string().contains("registry_tool_config"),
					"{mutation}: {error}"
				);
			}
		}
	}
}
/// The physical Registry guard must admit the personal OAuth transport contract,
/// even when data reaches the ORM without domain deserialization.
#[rstest]
#[tokio::test]
async fn database_admits_personal_transports_and_rejects_invalid_connection_fields(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let registry = registry(&database).await;
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	for transport in ["http", "mcp"] {
		let mut config = json!({"transport":transport,"endpoint":"https://example.com/mcp","replay":"read_only","secret_owner":"personal"});
		if transport == "mcp" {
			config["tool_name"] = json!("search");
			config["scopes"] = json!(["files:read"]);
		}
		let entry: Entry = serde_json::from_value(json!({
            "id":format!("personal-{transport}"),"version":"1.0.0","kind":"tool",
            "name":{"en":"Search"},"description":{"en":"Personal search"},
            "config":{"registry_deployment":"aidash://node","provider":format!("integration.{transport}@1"),"operation":"invoke","default_alias":"search","tier":"integration","transport":config}
        })).unwrap();
		// Register and read the actual persisted descriptor, including declared scopes.
		for (index, owner) in [Some("personal"), Some("any"), None]
			.into_iter()
			.enumerate()
		{
			let mut valid = entry.clone();
			valid.id = format!("valid-{transport}-{index}");
			if let Some(owner) = owner {
				valid.config["transport"]["secret_owner"] = json!(owner);
			} else {
				valid.config["transport"]
					.as_object_mut()
					.unwrap()
					.remove("secret_owner");
			}
			registry.register(&tenant, valid.clone()).await.unwrap();
			assert_eq!(
				registry
					.get(&tenant, &valid.id, &valid.version)
					.await
					.unwrap()
					.config,
				valid.config
			);
		}
		// Deliberately bypass domain validation so rejection must come from the DB.
		let mut invalid_fields = vec![
			("unknown-owner", "secret_owner", json!("shared")),
			("null-owner", "secret_owner", json!(null)),
			("numeric-owner", "secret_owner", json!(1)),
			("unknown-field", "credentials", json!("forbidden")),
		];
		if transport == "mcp" {
			invalid_fields.extend([
				("empty-scope", "scopes", json!([""])),
				("blank-scope", "scopes", json!([" "])),
				("non-string-scope", "scopes", json!([1])),
				("scalar-scopes", "scopes", json!("files:read")),
				("null-scopes", "scopes", json!(null)),
			]);
		} else {
			invalid_fields.push(("http-scopes", "scopes", json!(["files:read"])));
		}
		let mut connection = database.lease.handle();
		for (name, field, value) in invalid_fields {
			let mut invalid = entry.clone();
			invalid.id = format!("invalid-{transport}-{name}");
			invalid.config["transport"][field] = value;
			let definition = Definition::build()
				.tenant("acme")
				.id(&invalid.id)
				.version(&invalid.version)
				.kind(DefinitionKind::Tool)
				.metadata(serde_json::to_value(&invalid).unwrap().into())
				.finish();
			let error = Definition::objects()
				.create_with_conn(&mut connection, &definition)
				.await
				.unwrap_err();
			assert!(
				error.to_string().contains("registry_tool_config"),
				"{transport}/{name}: {error}"
			);
		}
	}
}

#[rstest]
#[tokio::test]
async fn context_descriptors_remain_valid_alongside_native_memory_roles(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let registry = registry(&database).await;
	for (kind, adapter) in [
		("memory", "conversation_memory"),
		("source", "workspace_retrieval"),
	] {
		let entry: Entry = serde_json::from_value(json!({
			"id":format!("descriptor-{kind}"),"version":"1.0.0","kind":kind,
			"name":{"en":"Context descriptor"},"description":{"en":"Context descriptor"},
			"config":{"schema_version":1,"source":{"adapter":adapter}}
		}))
		.unwrap();
		let saved = registry
			.register(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				entry.clone(),
			)
			.await
			.unwrap();
		assert_eq!(saved.config, entry.config);
		assert_eq!(
			registry
				.get(
					&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
					&saved.id,
					&saved.version
				)
				.await
				.unwrap(),
			saved
		);
		let mut ambiguous = entry;
		ambiguous.id = format!("ambiguous-{kind}");
		ambiguous.config["engine"] = json!("hindsight_rust");
		assert!(
			registry
				.register(
					&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
					ambiguous
				)
				.await
				.is_err()
		);
	}
}

#[rstest]
#[tokio::test]
async fn system_seed_is_discoverable_idempotent_and_rejects_owner_mutation(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let registry = registry(&database).await;
	registry.seed_system().await.unwrap();
	registry.seed_system().await.unwrap();
	let listed = registry
		.list(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			&Search {
				kind: Some("tool".into()),
				..Default::default()
			},
		)
		.await
		.unwrap();
	let operations = aidash_application::registry::system::operations().collect::<Vec<_>>();
	let validation = aidash_server::bootstrap::registry_validation();
	let offered: usize = aidash_application::registry::system::packages::offered(
		&validation,
		&validation,
		"aidash://node",
	)
	.unwrap()
	.iter()
	.map(|group| group.operations.len())
	.sum();
	let legacy = aidash_application::registry::system::legacy_workspace_entries(
		&validation,
		"aidash://node",
	)
	.unwrap()
	.len() + aidash_application::registry::system::legacy_journal_entries(
		&validation,
		"aidash://node",
	)
	.unwrap()
	.len();
	assert_eq!(listed.len(), operations.len() + offered + legacy);
	for operation in operations {
		let identity = QualifiedRef::builtin("aidash://node", operation);
		let entry = registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&identity.id,
				&identity.version,
			)
			.await
			.unwrap();
		let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
		assert_eq!(descriptor.operation, operation);
		assert!(entry.tags.iter().any(|tag| tag == "system"));
		assert!(
			registry
				.register(
					&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
					entry
				)
				.await
				.is_err()
		);
	}
	let definitions = registry
		.list(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			&Search::default(),
		)
		.await
		.unwrap()
		.len();
	let invalid:Entry=serde_json::from_value(json!({"id":"unsupported","version":"1.0.0","kind":"tool","name":{"en":"Test"},"description":{"en":"Test"},"config":{"registry_deployment":"aidash://node","provider":"integration.http@2","operation":"invoke","default_alias":"lookup","tier":"integration","transport":{"transport":"http","endpoint":"https://example.invalid","replay":"unsafe"}}})).unwrap();
	assert!(
		registry
			.register(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				invalid
			)
			.await
			.is_err()
	);
	assert_eq!(
		registry
			.list(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&Search::default()
			)
			.await
			.unwrap()
			.len(),
		definitions
	);
}
#[rstest]
#[tokio::test]
async fn existing_workspace_tool_versions_are_preserved_when_seeding_attachment_tools(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let registry = registry(&database).await;
	let old = aidash_application::registry::system::legacy_workspace_entries(
		&aidash_server::bootstrap::registry_validation(),
		"aidash://node",
	)
	.unwrap();
	let mut connection = database.lease.handle();
	for entry in &old {
		let definition = Definition::build()
			.tenant("acme")
			.id(&entry.id)
			.version(&entry.version)
			.kind(DefinitionKind::Tool)
			.metadata(serde_json::to_value(entry).unwrap().into())
			.finish();
		Definition::objects()
			.create_with_conn(&mut connection, &definition)
			.await
			.unwrap();
	}
	registry.seed_system().await.unwrap();
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	for entry in old {
		assert_eq!(
			registry
				.get(&tenant, &entry.id, &entry.version)
				.await
				.unwrap(),
			entry
		);
		assert_eq!(
			registry
				.get(&tenant, &entry.id, "1.0.1")
				.await
				.unwrap()
				.version,
			"1.0.1"
		);
	}
	let current = registry
		.get(&tenant, "aidash.workspace_read", "1.1.0")
		.await
		.unwrap();
	assert!(
		current.schema["properties"]["kind"]["enum"]
			.as_array()
			.unwrap()
			.contains(&json!("file"))
	);
	assert!(
		current.schema["properties"]["kind"]["enum"]
			.as_array()
			.unwrap()
			.contains(&json!("attachment"))
	);
}

/// Constraint DDL needs the table owner; the server role holds only DML.
async fn owner_ddl(database: &DatabaseFixture, sql: &str) {
	let owner = sqlx::PgPool::connect(&database.owner_url).await.unwrap();
	sqlx::raw_sql(sql).execute(&owner).await.unwrap();
	owner.close().await;
}

#[rstest]
#[tokio::test]
async fn develop_system_definitions_and_removed_decider_survive_upgrade_seeding(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let registry = registry(&database).await;
	let fixture = include_str!("../../../../../tests/fixtures/builtin_seed_develop_7abdf5.json")
		.replace("aidash://local", "aidash://node");
	let entries: Vec<Entry> = serde_json::from_str(&fixture).unwrap();
	// The snapshot uses a test catalog for native file and Skill tools. Their
	// native declarations did not change since develop; use the real catalog
	// so the database contains every deployed operation.
	let native = aidash_application::registry::system::entries(
		&aidash_server::bootstrap::registry_validation(),
		"aidash://node",
	)
	.unwrap();
	let entries = entries
		.into_iter()
		.map(|entry| {
			if entry
				.description
				.get("en")
				.is_some_and(|text| text.starts_with("Native "))
			{
				native
					.iter()
					.find(|current| current.id == entry.id && current.version == entry.version)
					.unwrap()
					.clone()
			} else {
				entry
			}
		})
		.collect::<Vec<_>>();
	let mut connection = database.lease.handle();
	// A deployed database held these rows before registry/0068 added its NOT
	// VALID check, so write them the way an upgrade meets them: with the check
	// absent, then restored without validating existing rows.
	owner_ddl(
		&database,
		include_str!(
			"../../../../migrations/registry/sql/backward/0068_reject_new_compaction_deciders.sql"
		),
	)
	.await;
	for entry in &entries {
		let definition = Definition::build()
			.tenant("acme")
			.id(&entry.id)
			.version(&entry.version)
			.kind(if entry.kind == "decider" {
				DefinitionKind::Decider
			} else {
				DefinitionKind::Tool
			})
			.metadata(serde_json::to_value(entry).unwrap().into())
			.finish();
		Definition::objects()
			.create_with_conn(&mut connection, &definition)
			.await
			.unwrap();
	}
	owner_ddl(
		&database,
		include_str!(
			"../../../../migrations/registry/sql/forward/0068_reject_new_compaction_deciders.sql"
		),
	)
	.await;
	registry.seed_system().await.unwrap();
	registry.seed_system().await.unwrap();
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	for entry in &entries {
		assert_eq!(
			registry
				.get(&tenant, &entry.id, &entry.version)
				.await
				.unwrap(),
			*entry
		);
	}
	for operation in ["subnode_task", "memory_mutate"] {
		let current = QualifiedRef::builtin("aidash://node", operation);
		assert_eq!(
			registry
				.get(&tenant, &current.id, &current.version)
				.await
				.unwrap()
				.version,
			"1.1.0"
		);
	}
}

#[rstest]
#[tokio::test]
async fn forged_reserved_bytes_abort_the_entire_seed(#[future] database: DatabaseFixture) {
	let database = database.await;
	let registry = registry(&database).await;
	let mut seed = aidash_application::registry::system::entries(
		&aidash_server::bootstrap::registry_validation(),
		"aidash://node",
	)
	.unwrap();
	let mut forged = seed.remove(0);
	forged.name.insert("en".into(), "Forged".into());
	let definition = Definition::build()
		.tenant("acme")
		.id(&forged.id)
		.version(&forged.version)
		.kind(DefinitionKind::Tool)
		.metadata(serde_json::to_value(&forged).unwrap().into())
		.finish();
	let mut connection = database.lease.handle();
	Definition::objects()
		.create_with_conn(&mut connection, &definition)
		.await
		.unwrap();
	assert!(registry.seed_system().await.is_err());
	let entries = registry
		.list(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			&Search::default(),
		)
		.await
		.unwrap();
	assert_eq!(entries.len(), 1);
	assert_eq!(entries[0], forged);
}
#[rstest]
#[tokio::test]
async fn bundle_members_must_exist_and_database_accepts_the_new_kind(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let registry = registry(&database).await;
	let mut bundle:Entry=serde_json::from_value(json!({"id":"package","version":"1.0.0","kind":"bundle","name":{"en":"Test"},"description":{"en":"Test"},"config":{"members":[{"registry_deployment":"aidash://node","id":"aidash.workspace_read","version":"1.0.0"}]}})).unwrap();
	assert!(
		registry
			.register(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				bundle.clone()
			)
			.await
			.is_err()
	);
	registry.seed_system().await.unwrap();
	let saved = registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			bundle.clone(),
		)
		.await
		.unwrap();
	assert_eq!(saved.kind, "bundle");
	assert_eq!(
		registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&saved.id,
				&saved.version
			)
			.await
			.unwrap(),
		saved
	);
	bundle.id = "foreign-package".into();
	bundle.config["members"][0]["registry_deployment"] = json!("aidash://foreign");
	assert!(
		registry
			.register(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				bundle
			)
			.await
			.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn bundle_member_id_ambiguity_is_rejected_by_registration_and_database(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let registry = registry(&database).await;
	registry.seed_system().await.unwrap();
	let member = QualifiedRef::builtin("aidash://node", "workspace_read");
	for (index, (node, version)) in [("aidash://node", "2.0.0"), ("aidash://foreign", "1.0.0")]
		.into_iter()
		.enumerate()
	{
		let duplicate = QualifiedRef {
			registry_deployment: node.into(),
			version: version.into(),
			..member.clone()
		};
		let bundle: Entry = serde_json::from_value(json!({"id":format!("ambiguous-{index}"),"version":"1.0.0","kind":"bundle","name":{"en":"Test"},"description":{"en":"Test"},"config":{"members":[member,duplicate]}})).unwrap();
		assert!(
			registry
				.register(
					&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
					bundle.clone()
				)
				.await
				.is_err()
		);
		let definition = Definition::build()
			.tenant("acme")
			.id(&bundle.id)
			.version(&bundle.version)
			.kind(DefinitionKind::Bundle)
			.metadata(serde_json::to_value(&bundle).unwrap().into())
			.finish();
		let mut connection = database.lease.handle();
		let error = Definition::objects()
			.create_with_conn(&mut connection, &definition)
			.await
			.unwrap_err();
		assert!(
			error.to_string().contains("registry_bundle_config"),
			"{error}"
		);
	}
}

#[rstest]
#[tokio::test]
async fn restart_seed_retains_definitions_during_pending_transaction_recovery(
	#[future] database: DatabaseFixture,
) {
	use aidash_server::apps::federation::transactions::models::{
		AtomicGate, AtomicParticipant, states::AtomicParticipantPhase,
	};
	let database = database.await;
	let registry = registry(&database).await;
	registry.seed_system().await.unwrap();
	let original = registry
		.list(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			&Search::default(),
		)
		.await
		.unwrap();
	let mut connection = database.lease.handle();
	let participant = AtomicParticipant::build()
		.coordinator("aidash://node")
		.digest("restart-seed-fixture")
		.manifest(json!({}).into())
		.phase(AtomicParticipantPhase::Reserved)
		.finish();
	AtomicParticipant::objects()
		.create_with_conn(&mut connection, &participant)
		.await
		.unwrap();
	let mut gate = AtomicGate::objects()
		.filter(AtomicGate::field_singleton().eq(true))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	gate.transaction_id = Some(participant.id);
	AtomicGate::objects()
		.update_with_conn(&mut connection, &gate)
		.await
		.unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(5), registry.seed_system())
		.await
		.expect("restart seeding must not wait on the transaction write gate")
		.unwrap();
	assert_eq!(
		registry
			.list(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&Search::default()
			)
			.await
			.unwrap(),
		original
	);
	let retained = AtomicGate::objects()
		.filter(AtomicGate::field_singleton().eq(true))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(retained.transaction_id, gate.transaction_id);
}
