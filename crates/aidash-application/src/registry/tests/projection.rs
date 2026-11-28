//! Native defaults and retirement at Registry admission.
use super::*;
use aidash_domain::{
	harness_contract::HarnessContract,
	registry::{bindings::AgentBindings, execution_contract},
};

const NODE: &str = "aidash://test";

/// An OpenRouter route with its Provider Credential: only such a route takes
/// cache breakpoints, so the caching rules below need one (#1169).
fn model_config() -> Value {
	json!({"provider":"openrouter","provider_credential":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}})
}

/// The Registry validation of a Deployment with a Provider Secret Store, which
/// the Provider Credential routes of [`model_config`] need.
#[fixture]
fn validation() -> DefinitionValidation {
	super::validation().with_provider_credentials(true)
}

fn agent_config(version: Option<&str>) -> Value {
	let mut config = json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"execute","bindings":[],"remove_default":[]});
	if let Some(version) = version {
		config["projection_version"] = json!(version);
	}
	config
}

fn scope_with(model: Value) -> Scope {
	let mut scope = Scope::default();
	scope.put_builtins(NODE);
	scope.put(definition("model", "model", model));
	scope
}

#[rstest]
#[case::legacy("projection_version", "legacy")]
#[case::ordered("projection_version", "ordered")]
#[case::old_contract("harness_contract", "legacy@1")]
#[tokio::test]
async fn registration_rejects_retired_agent_pins(
	validation: DefinitionValidation,
	#[case] field: &str,
	#[case] version: &str,
) {
	let mut scope = scope_with(model_config());
	let mut config = agent_config(None);
	config[field] = json!(version);
	let agent = definition("agent", "agent", config);
	let error = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap_err();
	let message = error.to_string();
	assert!(
		message.contains(version) && message.contains("retired"),
		"{message}"
	);
	assert!(!scope.entries.contains_key("agent@1.0.0"));
}

#[rstest]
#[tokio::test]
async fn new_agents_can_reference_stored_models_with_retired_fields(
	validation: DefinitionValidation,
) {
	let mut config = model_config();
	config["projection_versions"] = json!(["legacy", "ordered"]);
	let model = definition("model", "model", config.clone());
	let mut scope = scope_with(config.clone());
	let agent = definition("agent", "agent", agent_config(None));
	register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	assert_eq!(scope.entries["model@1.0.0"].config, config);
	assert_eq!(
		scope.entries["agent@1.0.0"].config["projection_version"],
		json!("native")
	);
	let error = register_definition(&mut Scope::default(), &validation, &model, NODE)
		.await
		.unwrap_err();
	assert!(
		error
			.to_string()
			.contains("projection_versions has been removed"),
		"{error}"
	);
}

/// Current serializations with no Projection Version selected.
const LEGACY_AGENT: &str = r#"{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"execute","bindings":[],"remove_default":[],"max_steps":64}"#;
const LEGACY_MODEL: &str = r#"{"provider":"openai_compatible","model_id":"vendor/model","endpoint":"https://model.example.test","reasoning_effort":null,"context_window":32768,"max_output_tokens":4096,"modalities":["text"],"media_routes":[],"cost":{}}"#;

#[rstest]
fn definitions_without_projection_fields_keep_their_bytes_and_resolve_as_native() {
	// Arrange
	let agent = agent_config(None);
	let model = json!({"provider":"openai_compatible","model_id":"vendor/model","endpoint":"https://model.example.test","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}});
	// Act
	let agent_bindings: AgentBindings = serde_json::from_value(agent).unwrap();
	let model_bindings: ModelConfig = serde_json::from_value(model).unwrap();
	// Assert
	assert_eq!(
		serde_json::to_string(&agent_bindings).unwrap(),
		LEGACY_AGENT
	);
	assert_eq!(
		digest(&serde_json::to_value(&agent_bindings).unwrap()),
		digest(&serde_json::from_str(LEGACY_AGENT).unwrap())
	);
	assert_eq!(
		serde_json::to_string(&model_bindings).unwrap(),
		LEGACY_MODEL
	);
	assert_eq!(
		digest(&serde_json::to_value(&model_bindings).unwrap()),
		digest(&serde_json::from_str(LEGACY_MODEL).unwrap())
	);
	assert_eq!(
		execution_contract(
			agent_bindings.projection_version,
			agent_bindings.harness_contract
		)
		.unwrap(),
		HarnessContract::Structured4
	);
}

/// A model on `model_id`, declaring `cache_mode` if given.
fn cache_model(model_id: &str, cache_mode: Option<&str>) -> Value {
	let mut config = model_config();
	config["model_id"] = json!(model_id);
	if let Some(mode) = cache_mode {
		config["cache_mode"] = json!(mode);
	}
	config
}

fn cache_agent(version: Option<&str>) -> Value {
	let mut config = agent_config(version);
	config["prompt_cache"] = json!("explicit");
	config
}

#[rstest]
#[case::off(Some("off"))]
#[case::omitted(None)]
#[case::explicit(Some("explicit"))]
#[tokio::test]
async fn registration_accepts_prompt_cache_on_an_explicit_model(
	validation: DefinitionValidation,
	#[case] prompt_cache: Option<&str>,
) {
	// Arrange
	let mut scope = scope_with(cache_model("anthropic/claude-fixture", Some("explicit")));
	let mut config = agent_config(Some("native"));
	if let Some(cache) = prompt_cache {
		config["prompt_cache"] = json!(cache);
	}
	let agent = definition("agent", "agent", config.clone());
	// Act
	let inserted = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	// Assert: only a Native Agent that names nothing gets the default (ADR 0115).
	assert!(inserted);
	assert_eq!(
		scope.entries["agent@1.0.0"].config.get("prompt_cache"),
		Some(&json!(prompt_cache.unwrap_or("explicit")))
	);
}

#[rstest]
#[case::undeclared_model("anthropic/claude-fixture", None, Some("native"))]
#[case::automatic_model("anthropic/claude-fixture", Some("automatic"), Some("native"))]
#[case::slug_outside_the_allowlist("openai/gpt-fixture", Some("explicit"), Some("native"))]
#[tokio::test]
async fn registration_rejects_explicit_prompt_cache_without_an_explicit_route(
	validation: DefinitionValidation,
	#[case] model_id: &str,
	#[case] cache_mode: Option<&str>,
	#[case] version: Option<&str>,
) {
	// Arrange: a stored model outside the allowlist bypasses model validation.
	let mut scope = scope_with(cache_model(model_id, cache_mode));
	let agent = definition("agent", "agent", cache_agent(version));
	// Act
	let error = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap_err();
	// Assert
	assert!(
		matches!(&error, Error::Domain(aidash_domain::Error::Invalid(message)) if message.starts_with("Agent prompt_cache explicit requires")),
		"{error:?}"
	);
	assert!(!scope.entries.contains_key("agent@1.0.0"));
	assert!(!scope.trace.iter().any(|step| step == "insert"));
}

#[rstest]
#[case::explicit_outside_the_allowlist("openai/gpt-fixture", "explicit", false)]
#[case::explicit_anthropic("anthropic/claude-fixture", "explicit", true)]
#[case::automatic("openai/gpt-fixture", "automatic", true)]
#[case::none("openai/gpt-fixture", "none", true)]
#[case::unknown("anthropic/claude-fixture", "always", false)]
fn model_definitions_admit_explicit_cache_mode_only_for_allowlisted_slugs(
	validation: DefinitionValidation,
	#[case] model_id: &str,
	#[case] cache_mode: &str,
	#[case] accepted: bool,
) {
	let model = definition("model", "model", cache_model(model_id, Some(cache_mode)));
	let result = validation.validate_in(&model, false);
	assert_eq!(result.is_ok(), accepted, "{result:?}");
	if cache_mode == "explicit" && !accepted {
		assert!(
			matches!(&result, Err(Error::Domain(aidash_domain::Error::Invalid(message))) if message.contains("cannot declare explicit prompt caching")),
			"{result:?}"
		);
	}
}

/// A Token Encoding is named from the closed list and never checked against
/// the slug, which Keyless endpoints choose freely (ADR 0017).
#[rstest]
#[case::bytes(json!("bytes"), true)]
#[case::calibrated(json!("calibrated"), true)]
#[case::o200k_base(json!({"tokenized": "o200k_base"}), true)]
#[case::encoding_unrelated_to_the_slug(json!({"tokenized": "gemma4"}), true)]
#[case::unlisted_encoding(json!({"tokenized": "cl100k_base"}), false)]
#[case::no_encoding(json!("tokenized"), false)]
#[case::exact(json!("exact"), false)]
fn model_definitions_name_only_listed_token_encodings(
	validation: DefinitionValidation,
	#[case] estimator: Value,
	#[case] accepted: bool,
) {
	let mut config = cache_model("openai/gpt-fixture", None);
	config["request_estimator"] = estimator;
	let model = definition("model", "model", config);
	let result = validation.validate_in(&model, false);
	assert_eq!(result.is_ok(), accepted, "{result:?}");
}

#[rstest]
fn definitions_without_cache_fields_keep_their_bytes() {
	let model: ModelConfig = serde_json::from_str(LEGACY_MODEL).unwrap();
	let agent: AgentBindings = serde_json::from_str(LEGACY_AGENT).unwrap();
	assert_eq!(model.cache_mode, None);
	assert_eq!(agent.prompt_cache, None);
	assert_eq!(serde_json::to_string(&model).unwrap(), LEGACY_MODEL);
	assert_eq!(serde_json::to_string(&agent).unwrap(), LEGACY_AGENT);
}

#[rstest]
#[tokio::test]
async fn registration_accepts_structured_harness_contracts_on_native(
	validation: DefinitionValidation,
) {
	// Arrange
	let contract = "structured@3";
	let mut scope = scope_with(model_config());
	let mut config = agent_config(Some("native"));
	config["harness_contract"] = json!(contract);
	let agent = definition("agent", "agent", config);
	// Act
	let inserted = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	// Assert
	assert!(inserted);
	let stored: AgentBindings =
		serde_json::from_value(scope.entries["agent@1.0.0"].config.clone()).unwrap();
	assert_eq!(
		execution_contract(stored.projection_version, stored.harness_contract).unwrap(),
		HarnessContract::Structured3
	);
}

/// `structured@1` and `structured@2` lost the #532 comparison and were removed.
#[rstest]
#[case::structured1("harness_contract", "structured@1", "structured@3, structured@4")]
#[case::structured2("harness_contract", "structured@2", "structured@3, structured@4")]
#[case::unknown_harness("harness_contract", "future@1", "structured@3, structured@4")]
#[case::unknown_projection("projection_version", "future", "native")]
#[tokio::test]
async fn registration_names_the_supported_values_for_removed_or_unknown_pins(
	validation: DefinitionValidation,
	#[case] field: &str,
	#[case] version: &str,
	#[case] allowed: &str,
) {
	// Arrange
	let mut scope = scope_with(model_config());
	let mut config = agent_config(Some("native"));
	config[field] = json!(version);
	let agent = definition("agent", "agent", config);
	// Act
	let error = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap_err();
	assert!(matches!(error, Error::Invalid(message)
		if message.contains(field) && message.contains(version) && message.contains(allowed)));
	assert!(!scope.entries.contains_key("agent@1.0.0"));
}

#[rstest]
#[case::omitted(None, None, "structured@4")]
#[case::explicit_native(Some("native"), None, "structured@4")]
#[case::explicit_contract(None, Some("structured@3"), "structured@3")]
#[tokio::test]
async fn new_agents_default_to_native_with_structured4(
	validation: DefinitionValidation,
	#[case] projection: Option<&str>,
	#[case] contract: Option<&str>,
	#[case] stored_contract: &str,
) {
	// Arrange
	let mut scope = scope_with(model_config());
	let mut config = agent_config(projection);
	if let Some(contract) = contract {
		config["harness_contract"] = json!(contract);
	}
	let agent = definition("agent", "agent", config);
	// Act
	register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	// Assert: the defaults are written into the stored definition.
	let stored = &scope.entries["agent@1.0.0"].config;
	assert_eq!(stored.get("projection_version"), Some(&json!("native")));
	assert_eq!(
		stored.get("harness_contract"),
		Some(&json!(stored_contract))
	);
}

#[tokio::test]
async fn replaying_a_model_stored_without_a_cache_mode_is_idempotent() {
	// Arrange: a model registered before ADR 0115 omits the field.
	let validation = validation();
	let mut config = model_config();
	config["model_id"] = json!("anthropic/claude-haiku-5.5");
	let mut scope = scope_with(config.clone());
	// Act
	let inserted = register_definition(
		&mut scope,
		&validation,
		&definition("model", "model", config),
		NODE,
	)
	.await
	.unwrap();
	// Assert
	assert!(!inserted);
	assert!(
		scope.entries["model@1.0.0"]
			.config
			.get("cache_mode")
			.is_none()
	);
}

/// A new model on a checked OpenRouter route declares explicit caching (ADR 0115).
#[rstest]
#[case::claude("anthropic/claude-haiku-5.5", None, Some("explicit"))]
#[case::gpt("openai/gpt-6.1-terra", None, Some("explicit"))]
#[case::gemini_caches_on_its_own("google/gemini-3.8-flash", None, None)]
#[case::older_gpt("openai/gpt-5.5", None, None)]
#[case::declared_off_is_kept("anthropic/claude-haiku-5.5", Some("none"), Some("none"))]
#[tokio::test]
async fn new_models_on_checked_routes_declare_explicit_caching(
	validation: DefinitionValidation,
	#[case] model_id: &str,
	#[case] declared: Option<&str>,
	#[case] stored: Option<&str>,
) {
	// Arrange
	let mut scope = Scope::default();
	scope.put_builtins(NODE);
	let mut config = model_config();
	config["model_id"] = json!(model_id);
	if let Some(declared) = declared {
		config["cache_mode"] = json!(declared);
	}
	// Act
	register_definition(
		&mut scope,
		&validation,
		&definition("model", "model", config),
		NODE,
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(
		scope.entries["model@1.0.0"].config.get("cache_mode"),
		stored.map(|mode| json!(mode)).as_ref()
	);
}

/// A new Native Agent caches when its model accepts breakpoints (ADR 0115).
#[rstest]
// model mode, projection, contract, declared => stored
#[case::native_default(Some("explicit"), None, None, None, Some("explicit"))]
#[case::model_without_breakpoints(Some("none"), None, None, None, None)]
#[case::model_omits_the_mode(None, None, None, None, None)]
#[case::declared_off_is_kept(Some("explicit"), None, None, Some("off"), Some("off"))]
#[tokio::test]
async fn new_native_agents_cache_when_their_model_accepts_breakpoints(
	validation: DefinitionValidation,
	#[case] model_mode: Option<&str>,
	#[case] projection: Option<&str>,
	#[case] contract: Option<&str>,
	#[case] declared: Option<&str>,
	#[case] stored: Option<&str>,
) {
	// Arrange
	let mut model = model_config();
	model["model_id"] = json!("anthropic/claude-haiku-5.5");
	if let Some(mode) = model_mode {
		model["cache_mode"] = json!(mode);
	}
	let mut scope = scope_with(model);
	let mut config = agent_config(projection);
	if let Some(contract) = contract {
		config["harness_contract"] = json!(contract);
	}
	if let Some(declared) = declared {
		config["prompt_cache"] = json!(declared);
	}
	// Act
	register_definition(
		&mut scope,
		&validation,
		&definition("agent", "agent", config),
		NODE,
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(
		scope.entries["agent@1.0.0"].config.get("prompt_cache"),
		stored.map(|mode| json!(mode)).as_ref()
	);
}

/// A definition stored before the #532 defaults omits the fields they fill;
/// replaying that registration stays idempotent instead of conflicting.
#[rstest]
#[case::omitted_projection(None, &["projection_version", "harness_contract"])]
#[case::native_without_contract(Some("native"), &["harness_contract"])]
#[tokio::test]
async fn replaying_a_definition_stored_without_defaults_is_idempotent(
	validation: DefinitionValidation,
	#[case] projection: Option<&str>,
	#[case] omitted: &[&str],
) {
	// Arrange: register, then strip the defaults as an older registration stored it.
	let mut scope = scope_with(model_config());
	let agent = definition("agent", "agent", agent_config(projection));
	register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	let stored = scope.entries.get_mut("agent@1.0.0").unwrap();
	for field in omitted {
		stored.config.as_object_mut().unwrap().remove(*field);
	}
	let before = stored.clone();
	// Act
	let inserted = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	// Assert
	assert!(!inserted);
	assert_eq!(scope.entries["agent@1.0.0"], before);
}

/// The registration response describes the Agent as stored, defaults included.
#[rstest]
#[tokio::test]
async fn registration_returns_the_defaulted_definition(validation: DefinitionValidation) {
	// Arrange
	let mut scope = scope_with(model_config());
	let agent = definition("agent", "agent", agent_config(None));
	// Act
	let returned = register(&mut scope, &validation, agent, None, true, NODE)
		.await
		.unwrap();
	// Assert
	assert_eq!(returned.config["projection_version"], json!("native"));
	assert_eq!(returned.config["harness_contract"], json!("structured@4"));
	assert_eq!(returned, scope.entries["agent@1.0.0"]);
}
