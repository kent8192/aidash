//! Native descriptor admission and immutable system seeding on disposable PostgreSQL.
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
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
	let store = Store::from_pool(pool, "aidash://node".into())
		.await
		.unwrap();
	Registry::new(store.pool.clone(), &store.node_id).unwrap()
}

#[rstest]
#[tokio::test]
async fn database_accepts_every_host_operation_and_its_exact_lifecycle(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let groups = aidash_application::registry::system::packages::declarations(
		&aidash_domain::identity::Principal::Operator,
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
						invalid.config["lifecycle"]["poll"]["registry_node"] =
							json!("aidash://foreign")
					}
					_ => invalid.config["lifecycle"]["extra"] = json!(true),
				}
				let definition = Definition::build()
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
		let saved = registry.register(entry.clone()).await.unwrap();
		assert_eq!(saved.config, entry.config);
		assert_eq!(
			registry.get(&saved.id, &saved.version).await.unwrap(),
			saved
		);
		let mut ambiguous = entry;
		ambiguous.id = format!("ambiguous-{kind}");
		ambiguous.config["engine"] = json!("hindsight_rust");
		assert!(registry.register(ambiguous).await.is_err());
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
		.list(&Search {
			kind: Some("tool".into()),
			..Default::default()
		})
		.await
		.unwrap();
	let operations = aidash_application::registry::system::operations().collect::<Vec<_>>();
	assert_eq!(listed.len(), operations.len());
	for operation in operations {
		let identity = QualifiedRef::builtin("aidash://node", operation);
		let entry = registry.get(&identity.id, &identity.version).await.unwrap();
		let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
		assert_eq!(descriptor.operation, operation);
		assert!(entry.tags.iter().any(|tag| tag == "system"));
		assert!(registry.register(entry).await.is_err());
	}
	let invalid:Entry=serde_json::from_value(json!({"id":"unsupported","version":"1.0.0","kind":"tool","name":{"en":"Test"},"description":{"en":"Test"},"config":{"registry_node":"aidash://node","provider":"integration.http@2","operation":"invoke","default_alias":"lookup","tier":"integration","transport":{"transport":"http","endpoint":"https://example.invalid","replay":"unsafe"}}})).unwrap();
	assert!(registry.register(invalid).await.is_err());
	assert_eq!(
		registry.list(&Search::default()).await.unwrap().len(),
		listed.len()
	);
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
	let entries = registry.list(&Search::default()).await.unwrap();
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
	let mut bundle:Entry=serde_json::from_value(json!({"id":"package","version":"1.0.0","kind":"bundle","name":{"en":"Test"},"description":{"en":"Test"},"config":{"members":[{"registry_node":"aidash://node","id":"aidash.workspace_read","version":"1.0.0"}]}})).unwrap();
	assert!(registry.register(bundle.clone()).await.is_err());
	registry.seed_system().await.unwrap();
	let saved = registry.register(bundle.clone()).await.unwrap();
	assert_eq!(saved.kind, "bundle");
	assert_eq!(
		registry.get(&saved.id, &saved.version).await.unwrap(),
		saved
	);
	bundle.id = "foreign-package".into();
	bundle.config["members"][0]["registry_node"] = json!("aidash://foreign");
	assert!(registry.register(bundle).await.is_err());
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
			registry_node: node.into(),
			version: version.into(),
			..member.clone()
		};
		let bundle: Entry = serde_json::from_value(json!({"id":format!("ambiguous-{index}"),"version":"1.0.0","kind":"bundle","name":{"en":"Test"},"description":{"en":"Test"},"config":{"members":[member,duplicate]}})).unwrap();
		assert!(registry.register(bundle.clone()).await.is_err());
		let definition = Definition::build()
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
	let original = registry.list(&Search::default()).await.unwrap();
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
	assert_eq!(registry.list(&Search::default()).await.unwrap(), original);
	let retained = AtomicGate::objects()
		.filter(AtomicGate::field_singleton().eq(true))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(retained.transaction_id, gate.transaction_id);
}
