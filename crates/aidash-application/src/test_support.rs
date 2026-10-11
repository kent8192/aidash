//! Small registered Binding graphs for portable adapter and lifecycle tests.
use crate::{
	Error, Result,
	ports::bindings::{BindingCatalog, ProviderCatalog},
};
use aidash_domain::{
	registry::{
		Entry, Projection,
		bindings::{BindingSnapshot, QualifiedRef},
	},
	tool::{
		ToolContract,
		providers::{ToolDescriptor, core_descriptor},
	},
};
use async_trait::async_trait;
use futures_util::FutureExt;
use serde_json::json;

pub(crate) fn entry(id: &str, kind: &str, config: serde_json::Value) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"schema":{"type":"object"},"config":config})).unwrap()
}
pub(crate) fn agent(id: &str) -> Entry {
	entry(
		id,
		"agent",
		json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Do the task.","bindings":[],"remove_default":[]}),
	)
}
struct Catalog {
	node: String,
	entries: Vec<Entry>,
}
#[async_trait]
impl BindingCatalog for Catalog {
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
		if reference.registry_node != self.node {
			return Err(Error::Forbidden);
		}
		if let Some(entry) = self
			.entries
			.iter()
			.find(|entry| entry.id == reference.id && entry.version == reference.version)
		{
			return Ok(entry.clone());
		}
		if let Some(operation) = reference.id.strip_prefix("aidash.") {
			return Ok(entry(
				&reference.id,
				"tool",
				serde_json::to_value(core_descriptor(&self.node, operation)?)?,
			));
		}
		Err(Error::NotFound(reference.id.clone()))
	}
	async fn installation(&mut self, _: &Projection) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn source(&mut self, _: &Entry) -> Result<()> {
		Ok(())
	}
}
struct Providers;
impl ProviderCatalog for Providers {
	fn contract(
		&self,
		descriptor: &ToolDescriptor,
		identity: &QualifiedRef,
	) -> Result<ToolContract> {
		Ok(descriptor.declared_contract(identity.clone())?)
	}
	fn implementation(&self, descriptor: &ToolDescriptor) -> Result<String> {
		Ok(format!("{}:portable-test", descriptor.provider))
	}
}
pub(crate) fn resolve(
	node: &str,
	root: &Entry,
	remote: bool,
	extras: Vec<Entry>,
) -> BindingSnapshot {
	let mut entries = extras;
	if !entries.iter().any(|entry| entry.kind == "model") {
		let model = &root.config["model"];
		let mut model_entry = entry(
			model["id"].as_str().unwrap(),
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://fixture.invalid","context_window":200000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}),
		);
		model_entry.version = model["version"].as_str().unwrap().into();
		entries.push(model_entry);
	}
	crate::registry::bindings::resolve(
		&mut Catalog {
			node: node.into(),
			entries,
		},
		&Providers,
		QualifiedRef {
			registry_node: node.into(),
			id: root.id.clone(),
			version: root.version.clone(),
		},
		root,
		remote,
	)
	.now_or_never()
	.expect("fixture catalog never performs I/O")
	.unwrap()
}
pub(crate) fn snapshot(node: &str, id: &str) -> BindingSnapshot {
	resolve(node, &agent(id), true, vec![])
}

/// An explicitly authored integration descriptor; publisher replay labels remain facts-free.
pub(crate) fn http_tool(node: &str, id: &str, alias: &str) -> Entry {
	entry(
		id,
		"tool",
		json!({"registry_node":node,"provider":"integration.http@1","operation":"invoke","default_alias":alias,"tier":"integration","transport":{"transport":"http","endpoint":"https://production.example/rpc","credential_env":null,"replay":"unsafe"}}),
	)
}
pub(crate) fn builtin_entries(node: &str) -> Vec<Entry> {
	crate::registry::system::operations()
		.map(|name| {
			entry(
				&format!("aidash.{name}"),
				"tool",
				json!(core_descriptor(node, name).unwrap()),
			)
		})
		.collect()
}
pub(crate) fn binding(kind: &str, node: &str, id: &str) -> serde_json::Value {
	json!({"kind":kind,"target":{"registry_node":node,"id":id,"version":"1.0.0"},"narrow":{}})
}

pub(crate) fn native_memory_entries() -> Vec<Entry> {
	let reference = |id: &str| json!({"id":id,"version":"1.0.0"});
	let zero = json!({"input_per_million":0,"output_per_million":0});
	vec![
		entry(
			"native-embedding",
			"embedding",
			json!({"provider":"openai","endpoint":"https://embedding.invalid","credential_env":null,"model":"fixture","model_version":"1","dimensions":3}),
		),
		entry("native-reranker", "reranker", json!({"provider":"rrf"})),
		entry(
			"native-tokenizer",
			"tokenizer",
			json!({"provider":"utf8_upper_bound"}),
		),
		entry(
			"native",
			"memory",
			json!({"engine":"hindsight_rust","policy":{
            "extraction":reference("fixture-model"),"derivation":reference("fixture-model"),"reflection":reference("fixture-model"),"embedding":reference("native-embedding"),"reranker":reference("native-reranker"),"tokenizer":reference("native-tokenizer"),
            "prices":{"extraction":zero,"derivation":zero,"reflection":zero,"embedding":zero,"reranker":zero},
            "retention":{"unit_max_age_days":null,"candidate_days":7,"history_days":30,"history_versions":16,"model_result_days":7,"backup_days":7,"purge_after_seconds":60,"purge_batch":32,"max_unit_records":128,"max_model_operations":1024},
            "bounds":{"max_unit_bytes":8192,"max_input_bytes":8192,"max_units":16,"max_candidates":8,"max_entities":8,"max_evidence":8,"max_links":8,"max_graph_hops":3,"max_graph_visits":32,"max_results":4,"max_context_tokens":8192,"max_model_calls":4,"max_model_tokens":8192,"max_cost_micros":10000,"max_retries":2,"max_call_seconds":30},
            "semantic_link_min_similarity_millionths":700000,"learn_from_runs":false,"maintain_observations":false,"refresh_mental_models":false}}),
		),
	]
}
pub(crate) fn native_memory_snapshot(node: &str, id: &str) -> BindingSnapshot {
	let mut root = agent(id);
	root.config["bindings"] = json!([binding("memory", node, "native")]);
	resolve(node, &root, true, native_memory_entries())
}
