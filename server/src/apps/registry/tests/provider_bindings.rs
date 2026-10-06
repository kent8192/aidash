//! Native descriptor admission and immutable system seeding on disposable PostgreSQL.
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
use aidash_domain::{
	registry::bindings::{DEFAULT_TOOLS, QualifiedRef, REQUIRED_TOOLS},
	tool::providers::ToolDescriptor,
};
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
	assert_eq!(listed.len(), REQUIRED_TOOLS.len() + DEFAULT_TOOLS.len());
	for operation in REQUIRED_TOOLS.iter().chain(DEFAULT_TOOLS) {
		let identity = QualifiedRef::builtin("aidash://node", operation);
		let entry = registry.get(&identity.id, &identity.version).await.unwrap();
		let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
		assert_eq!(descriptor.operation, *operation);
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
