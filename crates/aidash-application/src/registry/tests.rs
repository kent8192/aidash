use super::*;
use crate::ports::registry::*;
use aidash_domain::{
	model::ModelConfig,
	registry::{
		AgentConfig,
		rules::{digest, overlay_config},
	},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::Arc,
};

mod agent_plugins;
mod memory;
mod projection;

struct CoreContracts;
impl CoreToolCatalog for CoreContracts {
	fn specifications(
		&self,
		_: &aidash_domain::capabilities::CoreCapabilities,
	) -> BTreeMap<String, aidash_domain::provider::ToolSpec> {
		BTreeMap::new()
	}
}
#[fixture]
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(CoreContracts))
}
fn validate(entry: &Entry) -> Result<()> {
	validation().validate_in(entry, true)
}
fn skill_instructions(entry: &Entry) -> Result<String> {
	aidash_domain::registry::rules::skill_instructions(entry).map_err(Into::into)
}

#[derive(Default)]
struct Scope {
	entries: BTreeMap<String, Entry>,
	pages: Vec<(String, String, Value)>,
	generated: BTreeSet<String>,
	trace: Vec<String>,
	events: Vec<(String, Value)>,
	documents: Option<Value>,
	fail_documents: bool,
	secret_hosts: BTreeMap<String, Vec<String>>,
	plugins: BTreeMap<String, crate::ports::bindings::StoredAgentPlugin>,
}

#[async_trait]
impl PrivateKnowledgeRead for Scope {
	async fn documents(&mut self, _: &Entry) -> Result<Option<Value>> {
		self.trace.push("private_read".into());
		Ok(self.documents.clone())
	}
}
#[async_trait]
impl PrivateKnowledgeScope for Scope {
	async fn insert_documents(&mut self, _: &Entry, documents: Value) -> Result<()> {
		self.trace.push("private_documents".into());
		if self.fail_documents {
			return Err(Error::Conflict("private documents changed".into()));
		}
		self.documents = Some(documents);
		Ok(())
	}
}
fn identity(id: &str, version: &str) -> String {
	format!("{id}@{version}")
}
impl Scope {
	fn put(&mut self, entry: Entry) {
		self.entries
			.insert(identity(&entry.id, &entry.version), entry);
	}
	fn event(&mut self, kind: &str, payload: Value) {
		self.trace.push(kind.into());
		self.events.push((kind.into(), payload));
	}
}
#[async_trait]
impl DefinitionLookup for Scope {
	async fn agent_plugin(
		&mut self,
		digest: &str,
	) -> Result<Option<crate::ports::bindings::StoredAgentPlugin>> {
		Ok(self.plugins.get(digest).cloned())
	}
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		self.trace.push(format!("read:{id}@{version}"));
		self.entries
			.get(&identity(id, version))
			.cloned()
			.ok_or_else(|| Error::NotFound(id.into()))
	}
	async fn tenant_secret_hosts(&mut self, name: &str) -> Result<Option<Vec<String>>> {
		Ok(self.secret_hosts.get(name).cloned())
	}
}
#[async_trait]
impl DefinitionWriter for Scope {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool> {
		self.trace.push("insert".into());
		if let Some(existing) = self.entries.get(&identity(&entry.id, &entry.version)) {
			if existing != entry {
				return Err(Error::Conflict("immutable definition".into()));
			}
			return Ok(false);
		}
		self.put(entry.clone());
		Ok(true)
	}
}
#[async_trait]
impl RegistrationScope for Scope {
	async fn assign_id(&mut self, entry: &mut Entry, _: Option<Uuid>) -> Result<()> {
		self.trace.push("assign".into());
		if entry.id.is_empty() {
			entry.id = "assigned".into();
		}
		Ok(())
	}
	async fn authorize(&mut self, entry: &Entry) -> Result<()> {
		self.trace.push(format!("authorize:{}", entry.id));
		Ok(())
	}
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()> {
		self.event(kind, payload);
		Ok(())
	}
}
#[async_trait]
impl RegistryRead for Scope {
	async fn definitions(
		&mut self,
		_: Option<&str>,
		offset: usize,
		limit: Option<usize>,
	) -> Result<Vec<DefinitionDocument>> {
		Ok(self
			.pages
			.iter()
			.skip(offset)
			.take(limit.unwrap_or(usize::MAX))
			.map(|(id, version, metadata)| DefinitionDocument {
				id: id.clone(),
				version: version.clone(),
				metadata: metadata.clone(),
			})
			.collect())
	}
	async fn generated(&mut self, id: &str, version: &str) -> Result<bool> {
		Ok(self.generated.contains(&identity(id, version)))
	}
}
fn definition(id: &str, kind: &str, config: Value) -> Entry {
	let name = if id.is_empty() { "Fixture" } else { id };
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":name},"description":{"en":"fixture"},"config":config})).unwrap()
}
#[rstest]
#[tokio::test]
async fn registration_rejects_wrong_reference_kind_before_any_write(
	validation: DefinitionValidation,
) {
	let mut scope = Scope::default();
	scope.put(definition(
		"model",
		"skill",
		json!({"instructions":"not a model"}),
	));
	let agent = definition(
		"agent",
		"agent",
		json!({"schema_version":1,"bindings":[],"remove_default":[],"model":{"id":"model","version":"1.0.0"},"instructions":"execute"}),
	);
	let error = register_definition(&mut scope, &validation, &agent, "aidash://home")
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), "model must reference a model");
	assert_eq!(scope.trace, ["read:model@1.0.0"]);
	assert!(scope.events.is_empty());
}

#[rstest]
#[tokio::test]
async fn idempotent_registration_emits_one_event_and_preserves_input(
	validation: DefinitionValidation,
) {
	let mut scope = Scope::default();
	let mut input = definition(
		"skill",
		"skill",
		json!({"instructions":"Use the original evidence"}),
	);
	input.id.clear();
	let first = register(
		&mut scope,
		&validation,
		input.clone(),
		None,
		true,
		"aidash://home",
	)
	.await
	.unwrap();
	let replay = register(&mut scope, &validation, input, None, true, "aidash://home")
		.await
		.unwrap();
	assert_eq!(first, replay);
	assert_eq!(
		scope.trace,
		[
			"assign",
			"authorize:assigned",
			"insert",
			"registry.registered",
			"assign",
			"authorize:assigned",
			"insert"
		]
	);
	assert_eq!(
		scope.events,
		[(
			"registry.registered".into(),
			json!({"id":"assigned","version":"1.0.0","kind":"skill"})
		)]
	);
}

#[rstest]
#[case::remote(false)]
#[case::local(true)]
fn tool_registration_rejects_a_deployment_secret(
	validation: DefinitionValidation,
	#[case] local: bool,
) {
	for name in [json!("AIDASH_SECRET_BRAVE_SEARCH"), json!(null)] {
		let mut tool = crate::test_support::http_tool("aidash://local", "tool", "lookup");
		tool.config["transport"]["credential_env"] = name;
		let error = validation.validate_in(&tool, local).unwrap_err();
		assert!(
			error
				.to_string()
				.contains("cannot name a Deployment secret"),
			"{error}"
		);
	}
}

fn tool_with_secret(name: &str) -> Entry {
	let mut tool = crate::test_support::http_tool("aidash://home", "tool", "lookup");
	tool.config["transport"]["secret"] = json!(name);
	tool
}
#[rstest]
fn a_tenant_secret_is_refused_where_the_deployment_offers_none(validation: DefinitionValidation) {
	let tool = tool_with_secret("search");
	let error = validation.validate_in(&tool, true).unwrap_err();
	assert!(
		error
			.to_string()
			.contains("Tenant Secrets are not available"),
		"{error}"
	);
}
#[rstest]
#[case::malformed_name("Not A Name")]
fn tool_registration_validates_the_tenant_secret_name(
	validation: DefinitionValidation,
	#[case] name: &str,
) {
	let validation = validation.with_tenant_secrets(true);
	assert!(
		validation
			.validate_in(&tool_with_secret(name), true)
			.is_err()
	);
}
#[rstest]
#[tokio::test]
async fn registration_sends_a_tenant_secret_only_to_a_recorded_host(
	validation: DefinitionValidation,
) {
	let validation = validation.with_tenant_secrets(true);
	let tool = tool_with_secret("search");
	// No such Tenant Secret.
	let mut scope = Scope::default();
	assert!(
		register_definition(&mut scope, &validation, &tool, "aidash://home")
			.await
			.is_err()
	);
	// It exists, but records another host.
	scope
		.secret_hosts
		.insert("search".into(), vec!["other.example".into()]);
	let error = register_definition(&mut scope, &validation, &tool, "aidash://home")
		.await
		.unwrap_err();
	assert!(
		error.to_string().contains("does not record the host"),
		"{error}"
	);
	assert!(scope.entries.is_empty());
	// It records the endpoint's host.
	scope
		.secret_hosts
		.insert("search".into(), vec!["production.example".into()]);
	assert!(
		register_definition(&mut scope, &validation, &tool, "aidash://home")
			.await
			.unwrap()
	);
}

#[rstest]
#[case::remote(false)]
#[case::local(true)]
fn embedding_validation_needs_no_environment_credentials(#[case] local: bool) {
	let validation = DefinitionValidation::new(Arc::new(CoreContracts));
	let embedding = definition(
		"embedding",
		"embedding",
		json!({"provider":"openai","endpoint":"https://embedding.example.test","model":"text","model_version":"1","dimensions":3}),
	);
	// ADR 0006: embeddings name a Provider Catalog entry or no key at all.
	assert!(validation.validate_in(&embedding, local).is_ok());
}

#[rstest]
#[case::missing(
	0,
	"private documents are unavailable on this node; run the original agent on its owning node"
)]
#[case::changed(1, "private document digest mismatch")]
#[case::original(2, "")]
#[tokio::test]
async fn private_context_requires_the_original_node_content(
	#[case] state: u8,
	#[case] error: &str,
) {
	let documents =
		json!([{"name":"private.pdf","media_type":"application/pdf","text":"private evidence"}]);
	let entry = definition(
		"private",
		"source",
		json!({"schema_version":1,"source":{"adapter":"private_references","digest":aidash_domain::registry::knowledge::digest(&documents)}}),
	);
	let mut scope = Scope {
		documents: match state {
			0 => None,
			1 => Some(json!([])),
			_ => Some(documents.clone()),
		},
		..Default::default()
	};
	let result = personal::load(&mut scope, &entry).await;
	if error.is_empty() {
		assert_eq!(result.unwrap(), documents);
	} else {
		assert_eq!(result.unwrap_err().to_string(), error);
	}
	assert_eq!(scope.trace, ["private_read"]);
}

#[rstest]
#[case::accepted(false)]
#[case::changed_private_content(true)]
#[tokio::test]
async fn private_registration_emits_only_after_private_content_is_saved(
	validation: DefinitionValidation,
	#[case] fail_documents: bool,
) {
	let mut scope = Scope {
		fail_documents,
		..Default::default()
	};
	for builtin in crate::test_support::builtin_entries("aidash://home") {
		scope.put(builtin);
	}
	scope.put(definition("model", "model", json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://example.test","context_window":131072,"max_output_tokens":1024,"modalities":["text"],"cost":{}})));
	let entry = definition(
		"",
		"agent",
		json!({"schema_version":1,"bindings":[],"remove_default":[],"model":{"id":"model","version":"1.0.0"},"instructions":"Use the private evidence"}),
	);
	let draft = personal::validate_input(
		entry,
		vec![aidash_domain::registry::ReferenceDocument {
			name: "private.pdf".into(),
			media_type: "application/pdf".into(),
			text: "PRIVATE-TEXT".into(),
		}],
	)
	.unwrap();
	let prepared = personal::prepare(&mut scope, &validation, draft, "aidash://home")
		.await
		.unwrap();
	assert!(!json!(prepared.entry).to_string().contains("PRIVATE-TEXT"));
	scope.trace.clear();
	let result = personal::register(
		&mut scope,
		&validation,
		prepared,
		Uuid::new_v4(),
		"aidash://home",
	)
	.await;
	assert_eq!(result.is_ok(), !fail_documents);
	if fail_documents {
		assert_eq!(result.unwrap_err().to_string(), "private documents changed");
		assert_eq!(scope.events.len(), 0);
		assert_eq!(scope.trace.last().unwrap(), "private_documents");
	} else {
		let _entry = result.unwrap();
		assert_eq!(
			scope.trace[scope.trace.len() - 3..],
			["insert", "private_documents", "registry.registered"]
		);
		assert_eq!(
			scope.events,
			[(
				"registry.registered".into(),
				json!({"id":"assigned","version":"1.0.0","kind":"agent"})
			)]
		);
		assert_eq!(
			scope
				.entries
				.values()
				.find(|e| e.kind == "source")
				.unwrap()
				.config["source"]["digest"],
			aidash_domain::registry::knowledge::digest(scope.documents.as_ref().unwrap())
		);
	}
}

#[test]
fn run_media_routes_follow_current_evidence_and_model_modalities() {
	let now = chrono::Utc::now();
	let mut model: ModelConfig = serde_json::from_value(json!({
		"provider":"openrouter", "model_id":"fixture", "endpoint":"https://example.test",
		"context_window":8192, "modalities":["text","image","audio"],
		"cost":{}, "media_routes":[
			{"tag":"image", "formats":["image/png"], "source":"test", "verified_at":now - chrono::Duration::hours(1), "expires_at":now + chrono::Duration::hours(1)},
			{"tag":"audio", "formats":["wav"], "source":"test", "verified_at":now - chrono::Duration::hours(1), "expires_at":now + chrono::Duration::hours(1)},
			{"tag":"expired", "formats":["image/jpeg"], "source":"test", "verified_at":now - chrono::Duration::hours(2), "expires_at":now - chrono::Duration::hours(1)}
		]
	})).unwrap();
	assert_eq!(
		model.current_media_input_routes(),
		vec![vec!["image/png"], vec!["audio/wav", "audio/x-wav"],]
	);
	model.modalities = vec!["text".into()];
	assert!(model.current_media_input_routes().is_empty());
}

fn entry() -> Entry {
	serde_json::from_value(json!({"id":"research","version":"1.0.0","kind":"skill","name":{"en":"Research","ja":"調査"},"description":{"en":"Research"},"capabilities":["web.search"],"languages":["ja","en"],"config":{"instructions":"Research carefully"}})).unwrap()
}
#[rstest::fixture]
fn behavior_config(
	#[default(None)] creation: Option<bool>,
	#[default(None)] delegation: Option<bool>,
) -> AgentConfig {
	{
		let mut entry = crate::test_support::agent("fixture");
		let mut removed = vec![];
		if creation == Some(false) {
			removed.push("task_create");
		}
		if delegation == Some(false) {
			removed.push("task_delegate");
		}
		entry.config["remove_default"] = json!(removed);
		serde_json::from_value(entry.config).unwrap()
	}
}
#[rstest::rstest]
#[case(None, None, true, true)]
#[case(Some(true), Some(false), true, false)]
#[case(Some(false), Some(true), false, true)]
#[case(Some(false), Some(false), false, false)]
fn task_assignment_obeys_delegation_independently_of_creation(
	#[case] _creation: Option<bool>,
	#[case] _delegation: Option<bool>,
	#[case] creates: bool,
	#[case] delegates: bool,
	#[with(_creation, _delegation)] behavior_config: AgentConfig,
) {
	assert_eq!(behavior_config.permits_builtin("task_create"), creates);
	assert_eq!(behavior_config.permits_builtin("task_delegate"), delegates);
	assert!(!behavior_config.permits_builtin("task_assign"));
}
#[rstest::rstest]
fn conjunctive_search_and_localization() {
	let e = entry();
	validate(&e).unwrap();
	assert!(
		Search {
			capability: Some("web.search".into()),
			language: Some("JA".into()),
			..Default::default()
		}
		.matches(&e)
	);
	assert!(
		!Search {
			capability: Some("web.search".into()),
			language: Some("fr".into()),
			..Default::default()
		}
		.matches(&e)
	);
	assert!(
		Search {
			query: Some("調査".into()),
			..Default::default()
		}
		.matches(&e)
	);
}
#[rstest::rstest]
fn bundled_skill_files_have_bounded_safe_paths_and_are_listed_on_demand() {
	let mut e = entry();
	e.config = json!({"instructions":"Read the guide when relevant","files":[{"path":"references/guide.md","content":"Evidence"}]});
	validate(&e).unwrap();
	assert!(
		skill_instructions(&e)
			.unwrap()
			.contains("references/guide.md")
	);
	assert!(!skill_instructions(&e).unwrap().contains("Evidence"));
	e.config["files"] = json!([{"path":"assets/logo.png","content":"AAEC","encoding":"base64"}]);
	validate(&e).unwrap();
	assert!(
		skill_instructions(&e)
			.unwrap()
			.contains("binary, base64 encoded")
	);
	e.config["files"][0]["content"] = json!("not base64");
	assert!(validate(&e).is_err());
	e.config["files"][0]["content"] = json!("AAEC");
	e.config["files"][0]["encoding"] = json!("unknown");
	assert!(validate(&e).is_err());
	e.config["files"] = json!([{"path":"references/guide.md","content":"Evidence"}]);
	for path in [
		"../secret",
		"/absolute",
		"references/../secret",
		"references\\secret",
		"references/.hidden",
	] {
		e.config["files"][0]["path"] = json!(path);
		assert!(validate(&e).is_err(), "{path}");
	}
}
#[rstest::rstest]
fn skills_only_agents_do_not_need_custom_prompts() {
	let mut e = entry();
	e.kind = "agent".into();
	e.config = json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"bindings":[crate::test_support::binding("skill","aidash://home","research")],"remove_default":[]});
	assert!(validate(&e).is_ok());
	e.config["bindings"] = json!([]);
	assert!(validate(&e).is_err());
	e.config["instructions"] = json!("Legacy instructions");
	assert!(validate(&e).is_ok());
}
#[rstest::rstest]
fn rejects_invalid_metadata() {
	let mut e = entry();
	e.version = "latest".into();
	assert!(validate(&e).is_err());
	e.version = "1.0.0".into();
	e.id = "../../escape".into();
	assert!(validate(&e).is_err());
}
#[rstest::rstest]
fn unknown_requirements_and_invalid_clusters_are_rejected() {
	for value in [
		json!({"capabilty":"web.search"}),
		json!({"capabilities":["web.search"]}),
	] {
		assert!(serde_json::from_value::<Search>(value).is_err());
	}
	let mut e = entry();
	e.kind = "cluster".into();
	for config in [
		json!({}),
		json!({"coordinator":"research"}),
		json!({"coordinator":{"id":"research","version":"latest"}}),
	] {
		e.config = config;
		assert!(validate(&e).is_err());
	}
	e.config = json!({"coordinator":{"id":"research","version":"1.0.0"}});
	validate(&e).unwrap();
}
#[rstest::rstest]
fn installation_overrides_cannot_clear_a_personal_agent_knowledge_digest() {
	let mut config = json!({"knowledge_digest":"immutable-digest"});
	assert!(overlay_config(&mut config, &json!({"knowledge_digest":null})).is_err());
	assert_eq!(config["knowledge_digest"], "immutable-digest");
	assert!(overlay_config(&mut config, &json!({"display_name":"Local name"})).is_ok());
	assert_eq!(config["display_name"], "Local name");
}

#[rstest]
#[case("model")]
#[case("embedding")]
fn provider_credential_definition_sources_are_portable_and_fail_closed_without_store(
	#[case] kind: &str,
) {
	let config = if kind == "model" {
		json!({"provider":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1","provider_credential":"openrouter","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}})
	} else {
		json!({"provider":"openrouter","endpoint":"https://openrouter.ai/api/v1","provider_credential":"openrouter","model":"vendor/model","model_version":"1","dimensions":3})
	};
	let entry = definition(kind, kind, config);
	for local in [false, true] {
		assert!(
			validation()
				.validate_in(&entry, local)
				.unwrap_err()
				.to_string()
				.contains("Store is not configured")
		);
		let configured = validation().with_provider_credentials(true);
		configured.validate_in(&entry, local).unwrap();
		for invalid in [
			json!(Uuid::now_v7()),
			json!(format!("credential:{}", Uuid::now_v7())),
			json!("openai"),
			json!({"id":Uuid::now_v7()}),
		] {
			let mut changed = entry.clone();
			changed.config["provider_credential"] = invalid;
			assert!(configured.validate_in(&changed, local).is_err());
		}
		let mut changed = entry.clone();
		changed.config["credential_env"] = json!("AIDASH_SECRET_FALLBACK");
		assert!(configured.validate_in(&changed, local).is_err());
		changed = entry.clone();
		changed.config["endpoint"] = json!("https://attacker.test/api/v1");
		assert!(configured.validate_in(&changed, local).is_err());
	}
}

#[tokio::test]
async fn byok_timeout_is_validated_on_registration_import_and_consumption() {
	let validator = validation().with_provider_credentials(true);
	let mut entry = definition(
		"byok-timeout",
		"model",
		json!({"provider":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1","provider_credential":"openrouter","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
	);
	for timeout in [json!(null), json!(1), json!(3600)] {
		entry.config["request_timeout_secs"] = timeout;
		validator.validate_in(&entry, true).unwrap();
		validator.validate_in(&entry, false).unwrap();
	}
	for timeout in [3601_u32, u32::MAX] {
		entry.config["request_timeout_secs"] = json!(timeout);
		for local in [true, false] {
			assert!(
				validator
					.validate_in(&entry, local)
					.unwrap_err()
					.to_string()
					.contains("at most 3600")
			);
		}
		let mut scope = Scope::default();
		assert!(
			register_definition(&mut scope, &validator, &entry, "aidash://home")
				.await
				.is_err()
		);
		assert!(scope.trace.is_empty());
		assert!(scope.events.is_empty());
		assert!(scope.trace.is_empty());
	}
	// Direct env configurations keep their existing deadline range.
	entry
		.config
		.as_object_mut()
		.unwrap()
		.remove("provider_credential");
	validator.validate_in(&entry, true).unwrap();
}

#[rstest]
#[case("model", "model_id")]
#[case("embedding", "model")]
#[tokio::test]
async fn byok_model_scope_is_validated_before_registration_and_model_import(
	#[case] kind: &str,
	#[case] model_key: &str,
) {
	let validator = validation().with_provider_credentials(true);
	let config = if kind == "model" {
		json!({"provider":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1","provider_credential":"openrouter","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}})
	} else {
		json!({"provider":"openrouter","endpoint":"https://openrouter.ai/api/v1","provider_credential":"openrouter","model":"vendor/model","model_version":"1","dimensions":3})
	};
	let mut entry = definition("byok-model-scope", kind, config);
	entry.config[model_key] = json!("m".repeat(256));
	for local in [true, false] {
		validator.validate_in(&entry, local).unwrap();
	}
	for invalid in [
		"m".repeat(257),
		"vendor/../model".into(),
		"vendor//model".into(),
	] {
		entry.config[model_key] = json!(invalid);
		for local in [true, false] {
			assert!(
				validator
					.validate_in(&entry, local)
					.unwrap_err()
					.to_string()
					.contains("at most 256")
			);
		}
		let mut scope = Scope::default();
		assert!(
			register_definition(&mut scope, &validator, &entry, "aidash://home")
				.await
				.is_err()
		);
		assert!(scope.trace.is_empty());
		assert!(scope.events.is_empty());
	}
	// Existing direct-env definitions retain their prior model-ID contract.
	entry
		.config
		.as_object_mut()
		.unwrap()
		.remove("provider_credential");
	validator.validate_in(&entry, true).unwrap();
}

#[tokio::test]
async fn plan_models_need_enabled_plan_connections_and_plan_usage_limits() {
	let entry = definition(
		"chatgpt-model",
		"model",
		json!({"provider":"chatgpt","model_id":"gpt-fixture","endpoint":"https://api.openai.com/v1","plan_connection":"chatgpt","context_window":128000,"max_output_tokens":16384,"modalities":["text","image"],"cost":{},"projection_versions":["legacy","ordered","native"]}),
	);
	for local in [false, true] {
		assert!(
			validation()
				.with_provider_credentials(true)
				.validate_in(&entry, local)
				.unwrap_err()
				.to_string()
				.contains("Plan Connections are not enabled")
		);
		let enabled = validation().with_plan_connections(true);
		enabled.validate_in(&entry, local).unwrap();
		for (field, value) in [
			// ADR 0007: no Provider Credential serves the plan entry.
			("provider_credential", json!("openrouter")),
			("provider_credential", json!("chatgpt")),
			("plan_connection", json!("openrouter")),
			("provider", json!("openrouter")),
			("endpoint", json!("https://api.openai.com/v1/")),
			("endpoint", json!("https://attacker.test/v1")),
			("streaming", json!(false)),
			("modalities", json!(["text", "audio"])),
			("cache_mode", json!("explicit")),
			("model_id", json!("gpt/../fixture")),
		] {
			let mut changed = entry.clone();
			changed.config[field] = value;
			assert!(enabled.validate_in(&changed, local).is_err(), "{field}");
		}
	}
	// An OpenRouter route cannot name the plan entry either.
	let openrouter = definition(
		"openrouter-plan",
		"model",
		json!({"provider":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1","plan_connection":"chatgpt","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
	);
	assert!(
		validation()
			.with_plan_connections(true)
			.with_provider_credentials(true)
			.validate_in(&openrouter, true)
			.is_err()
	);
}
