//! One Workspace index requires compatible embeddings for every declared bank.
use super::*;

#[rstest]
#[case("same", true)]
#[case("same", false)]
#[case("dimensions", true)]
#[case("dimensions", false)]
#[case("model", true)]
#[case("model", false)]
#[case("model_version", true)]
#[case("endpoint", true)]
#[case("credential_env", true)]
#[tokio::test]
async fn agent_memory_sources_require_the_same_embedding_configuration(
	validation: DefinitionValidation,
	#[case] changed: &str,
	#[case] private: bool,
) {
	let mut scope = Scope::default();
	let reference = |id: &str| json!({"id":id,"version":"1.0.0"});
	scope.put(definition("model", "model", json!({"provider":"openai","model_id":"fixture","endpoint":"https://model.example.test","credential_env":null,"context_window":32768,"modalities":["text"],"cost":{}})));
	let embedding = json!({"provider":"openai","endpoint":"https://embedding.example.test","credential_env":null,"model":"text","model_version":"1","dimensions":3});
	let mut other = embedding.clone();
	if changed != "same" {
		other[changed] = match changed {
			"dimensions" => json!(4),
			"credential_env" => json!("AIDASH_SECRET_OTHER"),
			"endpoint" => json!("https://other.example.test"),
			_ => json!("different"),
		};
	}
	scope.put(definition("embedding-a", "embedding", embedding));
	scope.put(definition("embedding-b", "embedding", other));
	let zero = json!({"input_per_million":0,"output_per_million":0});
	for (name, role) in [("memory-a", "embedding-a"), ("memory-b", "embedding-b")] {
		let policy = json!({"engine":"hindsight_rust","policy":{
            "extraction":reference("model"),"derivation":reference("model"),"reflection":reference("model"),"embedding":reference(role),"reranker":reference("rrf"),"tokenizer":reference("tokens"),
            "prices":{"extraction":zero,"derivation":zero,"reflection":zero,"embedding":zero,"reranker":zero},
            "retention":{"unit_max_age_days":null,"candidate_days":7,"history_days":30,"history_versions":16,"model_result_days":7,"backup_days":7,"purge_after_seconds":60,"purge_batch":32,"max_unit_records":128,"max_model_operations":1024},
            "bounds":{"max_unit_bytes":8192,"max_input_bytes":8192,"max_units":16,"max_candidates":8,"max_entities":8,"max_evidence":8,"max_links":8,"max_graph_hops":3,"max_graph_visits":32,"max_results":4,"max_context_tokens":8192,"max_model_calls":4,"max_model_tokens":8192,"max_cost_micros":10000,"max_retries":2,"max_call_seconds":30},
            "semantic_link_min_similarity_millionths":700000,"learn_from_runs":false,"maintain_observations":false,"refresh_mental_models":false}});
		scope.put(definition(name, "memory", policy));
		scope.put(definition(
			&format!("source-{name}"),
			"source",
			json!({"scope":"workspace","memory":reference(name),"max_tokens":512}),
		));
	}
	let agent = definition(
		"agent",
		"agent",
		json!({"model":reference("model"),"instructions":"Use current memory","memory":private.then(||reference("memory-a")),"sources":if private {vec![reference("source-memory-b")]} else {vec![reference("source-memory-a"),reference("source-memory-b")]}}),
	);
	let result = register_definition(&mut scope, &validation, &agent, "aidash://home").await;
	if changed == "same" {
		assert!(result.unwrap());
		assert!(scope.entries.contains_key("agent@1.0.0"));
	} else {
		assert!(
			matches!(result, Err(Error::Invalid(message)) if message == "an Agent's memory providers require compatible embedding configurations")
		);
		assert!(!scope.entries.contains_key("agent@1.0.0"));
	}
}
