use crate::native_database::{DatabaseFixture, database};
use aidash_server::Error;
use aidash_server::apps::knowledge::models::{SemanticEntry, SemanticIndexe};
use aidash_server::authorization::identity::Actor;
use aidash_server::semantic::{
	ConfigureIndex, EmbeddingConfig, IndexSpec, PutEntry, Source, VectorConfig, service,
};
use aidash_server::store::Store;
use reinhardt::db::orm::{Json, Model};
use rstest::{fixture, rstest};
use serde_json::json;

#[rstest]
#[tokio::test]
async fn persisted_semantic_authority_and_revision_counters_remain_constrained(
	#[future] database: DatabaseFixture,
	spec: IndexSpec,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://limits".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Constraints", "Persisted authority")
		.await
		.unwrap();
	service::configure(
		&store,
		workspace.id,
		ConfigureIndex {
			expected_revision: 0,
			spec,
		},
	)
	.await
	.unwrap();
	let saved = service::put(
		&store,
		&Actor::Operator,
		workspace.id,
		memory("Source".into()),
	)
	.await
	.unwrap();
	let mut db = database.lease.handle();
	let original = SemanticEntry::objects()
		.get(saved.id)
		.get_with_db(&mut db)
		.await
		.unwrap();
	for invalid in [
		json!({}),
		json!([]),
		json!({"credential":null,"tenant":"","subject":"operator","subjects":[7]}),
		json!({"credential":"not-a-uuid","tenant":"","subject":"operator","subjects":[]}),
		json!({"credential":null,"tenant":7,"subject":"operator","subjects":[]}),
	] {
		assert!(
			SemanticEntry::objects()
				.filter(SemanticEntry::field_id().eq(saved.id))
				.update_fields_with_conn(
					&mut db,
					[(SemanticEntry::field_authority(), Json(invalid))]
				)
				.await
				.is_err()
		);
	}
	assert!(
		SemanticIndexe::objects()
			.filter(SemanticIndexe::field_revision().eq(1))
			.update_fields_with_conn(&mut db, [(SemanticIndexe::field_revision(), i64::MAX)])
			.await
			.is_err()
	);
	assert!(
		SemanticEntry::objects()
			.filter(SemanticEntry::field_id().eq(saved.id))
			.update_fields_with_conn(&mut db, [(SemanticEntry::field_revision(), 0_i64)])
			.await
			.is_err()
	);
	let unchanged = SemanticEntry::objects()
		.get(saved.id)
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(unchanged.authority, original.authority);
	assert_eq!(unchanged.revision, 1);
	assert_eq!(
		service::get_index(&store, &Actor::Operator, workspace.id)
			.await
			.unwrap()
			.revision,
		1
	);
}

#[fixture]
fn spec() -> IndexSpec {
	IndexSpec {
		embedding: EmbeddingConfig {
			provider: "openai".into(),
			endpoint: "http://localhost:9".into(),
			credential_env: None,
			provider_credential: None,
			model: "fixture".into(),
			model_version: "1".into(),
			dimensions: 8,
		},
		vector: VectorConfig {
			provider: "postgres".into(),
			endpoint: "local".into(),
			credential_env: None,
		},
		enabled: true,
		auto_context: false,
		max_sources: 10,
		max_results: 5,
		max_result_tokens: 128,
		max_input_bytes: 256,
	}
}

fn memory(text: String) -> PutEntry {
	PutEntry {
		key: "memory".into(),
		expected_revision: 0,
		source: Source::Memory { text },
		agent: None,
		metadata: json!({}),
	}
}

#[rstest]
#[tokio::test]
async fn memory_and_index_limits_are_enforced_without_triggers(
	#[future] database: DatabaseFixture,
	mut spec: IndexSpec,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://limits".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Limits", "Verify memory limits")
		.await
		.unwrap();
	service::configure(
		&store,
		workspace.id,
		ConfigureIndex {
			expected_revision: 0,
			spec: spec.clone(),
		},
	)
	.await
	.unwrap();

	// Japanese text exercises byte length rather than character count.
	let oversized = service::put(
		&store,
		&Actor::Operator,
		workspace.id,
		memory("あ".repeat(86)),
	)
	.await;
	assert!(matches!(oversized, Err(Error::Invalid(_))), "{oversized:?}");
	assert!(
		service::entries(&store, &Actor::Operator, workspace.id)
			.await
			.unwrap()
			.is_empty()
	);
	let saved = service::put(
		&store,
		&Actor::Operator,
		workspace.id,
		memory("あ".repeat(80)),
	)
	.await
	.unwrap();

	spec.max_input_bytes = 128;
	let reduced = service::configure(
		&store,
		workspace.id,
		ConfigureIndex {
			expected_revision: 1,
			spec: spec.clone(),
		},
	)
	.await;
	assert!(matches!(reduced, Err(Error::Conflict(_))), "{reduced:?}");
	let unchanged = service::get_index(&store, &Actor::Operator, workspace.id)
		.await
		.unwrap();
	assert_eq!(unchanged.revision, 1);
	assert_eq!(unchanged.configuration().unwrap().max_input_bytes, 256);
	assert_eq!(
		service::entries(&store, &Actor::Operator, workspace.id)
			.await
			.unwrap()[0]
			.id,
		saved.id
	);

	spec.max_input_bytes = 240;
	let equal = service::configure(
		&store,
		workspace.id,
		ConfigureIndex {
			expected_revision: 1,
			spec,
		},
	)
	.await
	.unwrap();
	assert_eq!(equal.revision, 2);
	assert_eq!(equal.configuration().unwrap().max_input_bytes, 240);
}

#[rstest]
#[tokio::test]
async fn concurrent_memory_write_and_limit_reduction_preserve_the_limit(
	#[future] database: DatabaseFixture,
	mut spec: IndexSpec,
) {
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://limits".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Race", "Serialize limit changes")
		.await
		.unwrap();
	service::configure(
		&store,
		workspace.id,
		ConfigureIndex {
			expected_revision: 0,
			spec: spec.clone(),
		},
	)
	.await
	.unwrap();
	spec.max_input_bytes = 128;
	let (write, reduce) = tokio::join!(
		service::put(
			&store,
			&Actor::Operator,
			workspace.id,
			memory("x".repeat(240))
		),
		service::configure(
			&store,
			workspace.id,
			ConfigureIndex {
				expected_revision: 1,
				spec
			}
		),
	);
	assert_ne!(
		write.is_ok(),
		reduce.is_ok(),
		"exactly one operation must win: {write:?}, {reduce:?}"
	);
	let index = service::get_index(&store, &Actor::Operator, workspace.id)
		.await
		.unwrap();
	let limit = index.configuration().unwrap().max_input_bytes;
	for entry in service::entries(&store, &Actor::Operator, workspace.id)
		.await
		.unwrap()
	{
		let Source::Memory { text } = serde_json::from_value(entry.source).unwrap() else {
			panic!("memory source")
		};
		assert!(text.len() <= limit);
	}
}

#[rstest]
#[tokio::test]
async fn operator_vectors_accept_empty_tenants_and_remain_workspace_scoped(
	#[future] database: DatabaseFixture,
	spec: IndexSpec,
) {
	use aidash_application::ports::VectorIndex;
	use aidash_domain::semantic::VectorFilter;
	use uuid::Uuid;
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://vectors".into(),
	)
	.await
	.unwrap();
	let workspace = store
		.create_workspace("Operator vectors", "Empty tenant remains scoped")
		.await
		.unwrap();
	let transport = aidash_server::bootstrap::semantic_transport(&store);
	let point = Uuid::now_v7();
	let values = vec![1.0; spec.embedding.dimensions];
	transport
		.ensure_collection(&spec.vector, "operator-fixture", values.len())
		.await
		.unwrap();
	transport
		.upsert(
			&spec.vector,
			"operator-fixture",
			point,
			&values,
			json!({"workspace_id":workspace.id,"tenant":""}),
		)
		.await
		.unwrap();
	for (scope, expected) in [(workspace.id, 1), (Uuid::now_v7(), 0)] {
		let found = transport
			.query(
				&spec.vector,
				"operator-fixture",
				&values,
				VectorFilter {
					allowed: &[point],
					workspace: scope,
					tenant: "",
				},
				1,
			)
			.await
			.unwrap();
		assert_eq!(found.len(), expected);
	}
}
