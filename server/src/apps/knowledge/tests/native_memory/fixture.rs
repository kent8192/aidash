//! Shared native-memory fixture; integration and repository tests use the same ownership.
use super::DatabaseFixture;
use aidash_domain::{memory::*, registry::EntityRef};
use aidash_server::{
	apps::identity::models::{AuthorizationBundle, AuthorizationWorkspace},
	registry::{Entry, Registry},
	store::Store,
};
use reinhardt::db::orm::{Json, Model};
use rstest::fixture;
use serde_json::json;
use uuid::Uuid;
pub(super) fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
pub(super) fn replace_memory_binding(agent: &mut Entry, provider: EntityRef) {
	agent.binding_normalization = None;
	let binding = agent.config["bindings"]
		.as_array_mut()
		.unwrap()
		.iter_mut()
		.find(|binding| binding["kind"] == "memory")
		.unwrap();
	binding["target"] = json!({"registry_node":"aidash://native-memory","id":provider.id,"version":provider.version});
}
pub(super) fn entry(kind: &str, id: &str, mut config: serde_json::Value) -> Entry {
	if kind == "agent" {
		let mut bindings = vec![];
		if let Some(provider) = config.get("memory").filter(|value| !value.is_null()) {
			bindings.push(json!({"kind":"memory","target":{"registry_node":"aidash://native-memory","id":provider["id"],"version":provider["version"]},"narrow":{}}));
		}
		for source in config
			.get("sources")
			.and_then(|v| v.as_array())
			.into_iter()
			.flatten()
		{
			bindings.push(json!({"kind":"source","target":{"registry_node":"aidash://native-memory","id":source["id"],"version":source["version"]},"narrow":{}}));
		}
		config = json!({"schema_version":1,"model":config["model"],"instructions":config["instructions"],"bindings":bindings,"remove_default":["file_search","file_read"]});
	}
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id,"ja":id},"description":{"en":"fixture"},
		"capabilities":[],"languages":["en","ja"],"config":config})).unwrap()
}
#[fixture]
pub(super) fn bounds() -> Bounds {
	Bounds {
		max_unit_bytes: 8192,
		max_input_bytes: 16384,
		max_units: 16,
		max_candidates: 8,
		max_entities: 8,
		max_evidence: 8,
		max_links: 8,
		max_graph_hops: 3,
		max_graph_visits: 32,
		max_results: 4,
		max_context_tokens: 8192,
		max_model_calls: 4,
		max_model_tokens: 8192,
		max_cost_micros: 10000,
		max_retries: 2,
		max_call_seconds: 30,
	}
}
pub(super) async fn setup(database: &DatabaseFixture, bounds: Bounds) -> (Store, Registry, Uuid) {
	setup_endpoint(database, bounds, "http://127.0.0.1:9/v1").await
}
pub(super) async fn setup_endpoint(
	database: &DatabaseFixture,
	bounds: Bounds,
	endpoint: &str,
) -> (Store, Registry, Uuid) {
	setup_endpoint_flags(database, bounds, endpoint, (false, false, false)).await
}
pub(super) async fn setup_endpoint_flags(
	database: &DatabaseFixture,
	bounds: Bounds,
	endpoint: &str,
	flags: (bool, bool, bool),
) -> (Store, Registry, Uuid) {
	setup_endpoint_retention(database, bounds, endpoint, flags, None).await
}
pub(super) async fn setup_endpoint_retention(
	database: &DatabaseFixture,
	bounds: Bounds,
	endpoint: &str,
	flags: (bool, bool, bool),
	unit_max_age_days: Option<u32>,
) -> (Store, Registry, Uuid) {
	setup_endpoint_decay(database, bounds, endpoint, flags, unit_max_age_days, None).await
}
pub(super) async fn setup_endpoint_decay(
	database: &DatabaseFixture,
	bounds: Bounds,
	endpoint: &str,
	flags: (bool, bool, bool),
	unit_max_age_days: Option<u32>,
	decay: Option<Decay>,
) -> (Store, Registry, Uuid) {
	let store = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		"aidash://native-memory".into(),
	)
	.await
	.unwrap();
	let store = aidash_server::semantic::services::memory_recovery::initialize(
		&store,
		database.recovery_directory.path().to_owned(),
	)
	.await
	.unwrap();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	registry.seed_system().await.unwrap();
	for (kind, id, config) in [
		(
			"model",
			"m",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":endpoint,"credential_env":null,
			"context_window":32768,"max_output_tokens":8192,"modalities":["text"],"cost":{}}),
		),
		(
			"embedding",
			"e",
			json!({"provider":"openai","endpoint":endpoint,"credential_env":null,"model":"fixture","model_version":"1.0.0","dimensions":3}),
		),
		("reranker", "r", json!({"provider":"rrf"})),
		("tokenizer", "t", json!({"provider":"utf8_upper_bound"})),
		(
			"memory",
			"p",
			serde_json::to_value(ProviderConfig {
				engine: EngineKind::HindsightRust,
				policy: Policy {
					extraction: reference("m"),
					derivation: reference("m"),
					reflection: reference("m"),
					embedding: reference("e"),
					reranker: reference("r"),
					tokenizer: reference("t"),
					semantic_link_min_similarity_millionths: 700_000,
					decay,
					retention: Retention {
						unit_max_age_days,
						candidate_days: 7,
						history_days: 30,
						history_versions: 16,
						model_result_days: 7,
						backup_days: 7,
						purge_after_seconds: 60,
						purge_batch: 32,
						max_unit_records: 128.max(bounds.max_units),
						max_model_operations: 1024,
					},
					prices: Prices {
						extraction: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						derivation: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						reflection: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						embedding: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						reranker: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
					},
					bounds,
					learn_from_runs: flags.0,
					maintain_observations: flags.1,
					refresh_mental_models: flags.2,
				},
			})
			.unwrap(),
		),
		(
			"agent",
			"a",
			json!({"model":reference("m"),"instructions":"Use current evidence","tools":[],"skills":[],"memory":reference("p"),"allow_memory_write":true}),
		),
	] {
		registry.register(entry(kind, id, config)).await.unwrap();
	}
	let workspace = store
		.create_workspace("Memory", "Keep current evidence")
		.await
		.unwrap();
	let mut db = database.lease.handle();
	let bundle: aidash_domain::policy::PolicyBundle =
		serde_json::from_value(json!({"tenant":"acme"})).unwrap();
	AuthorizationBundle::objects()
		.create_with_conn(
			&mut db,
			&AuthorizationBundle::build()
				.tenant("acme")
				.revision(1)
				.document(Json(serde_json::to_value(bundle).unwrap()))
				.finish(),
		)
		.await
		.unwrap();
	AuthorizationWorkspace::objects()
		.create_with_conn(
			&mut db,
			&AuthorizationWorkspace::build()
				.workspace_id(workspace.id)
				.tenant("acme")
				.owner_subject("operator")
				.finish(),
		)
		.await
		.unwrap();
	aidash_server::semantic::service::configure(
		&store,
		workspace.id,
		aidash_server::semantic::ConfigureIndex {
			expected_revision: 0,
			spec: aidash_server::semantic::IndexSpec {
				embedding: serde_json::from_value(json!({"provider":"openai","endpoint":endpoint,"credential_env":null,"model":"fixture","model_version":"1.0.0","dimensions":3})).unwrap(),
				vector: aidash_domain::semantic::VectorConfig { provider: "postgres".into(), endpoint: "local".into(), credential_env: None },
				enabled: true, auto_context: false, max_sources: 64, max_results: 4,
				max_result_tokens: 8192, max_input_bytes: 16384,
			},
		},
	).await.unwrap();
	(store, registry, workspace.id)
}
pub(super) fn content(text: &str) -> Content {
	Content {
		mental_model: None,
		text: text.into(),
		kind: Kind::World,
		learning: Learning::Fact,
		verification: Verification::Unverified,
		occurred: None,
		entities: vec![],
		evidence: vec![],
		links: vec![],
	}
}
pub(super) fn mutation(bank: &Bank, change: Change) -> Mutation {
	Mutation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		changes: vec![change],
	}
}
