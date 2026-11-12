//! Agents pin only implemented Projection Versions their model declares.
use super::*;
use aidash_domain::{projection::ProjectionVersion, registry::bindings::AgentBindings};

const NODE: &str = "aidash://test";

fn model_config(versions: Option<Value>) -> Value {
	let mut config = json!({"provider":"openrouter","model_id":"vendor/model","endpoint":"https://model.example.test","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}});
	if let Some(versions) = versions {
		config["projection_versions"] = versions;
	}
	config
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

fn is_invalid(error: &Error) -> bool {
	matches!(
		error,
		Error::Invalid(_) | Error::Domain(aidash_domain::Error::Invalid(_))
	)
}

#[rstest]
#[case::declared_ordered(Some(json!(["legacy","ordered"])), Some("ordered"))]
#[case::declared_legacy(Some(json!(["legacy","ordered"])), Some("legacy"))]
#[case::declared_native(Some(json!(["legacy","ordered","native"])), Some("native"))]
#[case::native_only(Some(json!(["native"])), Some("native"))]
#[case::undeclared_legacy(None, None)]
#[case::explicit_legacy(None, Some("legacy"))]
#[tokio::test]
async fn registration_accepts_versions_the_model_declares(
	validation: DefinitionValidation,
	#[case] versions: Option<Value>,
	#[case] version: Option<&str>,
) {
	// Arrange
	let mut scope = scope_with(model_config(versions));
	let agent = definition("agent", "agent", agent_config(version));
	// Act
	let inserted = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	// Assert
	assert!(inserted);
	let stored: AgentBindings =
		serde_json::from_value(scope.entries["agent@1.0.0"].config.clone()).unwrap();
	assert_eq!(
		stored.projection_version.unwrap_or_default(),
		version.map_or(ProjectionVersion::Legacy, |name| {
			serde_json::from_value(json!(name)).unwrap()
		})
	);
}

#[rstest]
#[case::undeclared_ordered(None, "ordered")]
#[case::ordered_not_listed(Some(json!(["legacy"])), "ordered")]
#[case::legacy_not_listed(Some(json!(["ordered"])), "legacy")]
#[case::native_not_listed(Some(json!(["legacy","ordered"])), "native")]
#[case::undeclared_native(None, "native")]
#[tokio::test]
async fn registration_rejects_versions_the_model_cannot_render(
	validation: DefinitionValidation,
	#[case] versions: Option<Value>,
	#[case] version: &str,
) {
	// Arrange
	let mut scope = scope_with(model_config(versions));
	let agent = definition("agent", "agent", agent_config(Some(version)));
	// Act
	let error = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap_err();
	// Assert
	assert!(is_invalid(&error), "{error:?}");
	assert!(!scope.entries.contains_key("agent@1.0.0"));
	assert!(!scope.trace.iter().any(|step| step == "insert"));
}

#[rstest]
#[case::duplicate(json!(["legacy","ordered","legacy"]))]
#[case::unknown(json!(["legacy","v4"]))]
fn model_definitions_reject_unrenderable_declarations(
	validation: DefinitionValidation,
	#[case] versions: Value,
) {
	let model = definition("model", "model", model_config(Some(versions)));
	let error = validation.validate_in(&model, false).unwrap_err();
	assert!(is_invalid(&error), "{error:?}");
}

#[rstest]
#[case::omitted(None)]
#[case::legacy_and_ordered(Some(json!(["legacy","ordered"])))]
#[case::ordered_only(Some(json!(["ordered"])))]
#[case::with_native(Some(json!(["legacy","ordered","native"])))]
fn model_definitions_accept_implemented_declarations(
	validation: DefinitionValidation,
	#[case] versions: Option<Value>,
) {
	let model = definition("model", "model", model_config(versions));
	validation.validate_in(&model, false).unwrap();
}

/// Current serializations with no Projection Version selected.
const LEGACY_AGENT: &str = r#"{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"execute","bindings":[],"remove_default":[],"max_steps":64}"#;
const LEGACY_MODEL: &str = r#"{"provider":"openrouter","model_id":"vendor/model","endpoint":"https://model.example.test","reasoning_effort":null,"context_window":32768,"max_output_tokens":4096,"modalities":["text"],"media_routes":[],"cost":{}}"#;

#[rstest]
fn definitions_without_projection_fields_serialize_and_resolve_as_legacy() {
	// Arrange
	let agent = agent_config(None);
	let model = model_config(None);
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
		agent_bindings.projection_version.unwrap_or_default(),
		ProjectionVersion::Legacy
	);
	assert!(model_bindings.supports_projection(ProjectionVersion::Legacy));
	assert!(!model_bindings.supports_projection(ProjectionVersion::Ordered));
}

/// An Ordered-capable model on `model_id`, declaring `cache_mode` if given.
fn cache_model(model_id: &str, cache_mode: Option<&str>) -> Value {
	let mut config = model_config(Some(json!(["legacy", "ordered", "native"])));
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
async fn registration_accepts_prompt_cache_on_an_ordered_agent_and_explicit_model(
	validation: DefinitionValidation,
	#[case] prompt_cache: Option<&str>,
	#[values("ordered", "native")] version: &str,
) {
	// Arrange
	let mut scope = scope_with(cache_model("anthropic/claude-fixture", Some("explicit")));
	let mut config = agent_config(Some(version));
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
	let expected = match (prompt_cache, version) {
		(None, "native") => Some(json!("explicit")),
		_ => config.get("prompt_cache").cloned(),
	};
	assert_eq!(
		scope.entries["agent@1.0.0"].config.get("prompt_cache"),
		expected.as_ref()
	);
}

#[rstest]
#[case::undeclared_model("anthropic/claude-fixture", None, Some("ordered"))]
#[case::automatic_model("anthropic/claude-fixture", Some("automatic"), Some("ordered"))]
#[case::slug_outside_the_allowlist("openai/gpt-fixture", Some("explicit"), Some("ordered"))]
// Without a Projection Version a Native-capable model now defaults to Native (#532),
// so the Legacy agent names it.
#[case::legacy_agent("anthropic/claude-fixture", Some("explicit"), Some("legacy"))]
#[tokio::test]
async fn registration_rejects_explicit_prompt_cache_without_ordered_and_an_explicit_route(
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
#[case::legacy(Some("legacy"), "structured@3")]
#[case::ordered(Some("ordered"), "structured@3")]
#[tokio::test]
async fn registration_rejects_structured_harness_contracts_without_native(
	validation: DefinitionValidation,
	#[case] version: Option<&str>,
	#[case] contract: &str,
) {
	// Arrange
	let mut scope = scope_with(model_config(Some(json!(["legacy", "ordered", "native"]))));
	let mut config = agent_config(version);
	config["harness_contract"] = json!(contract);
	let agent = definition("agent", "agent", config);
	// Act
	let error = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap_err();
	// Assert
	assert!(is_invalid(&error), "{error:?}");
	assert!(!scope.entries.contains_key("agent@1.0.0"));
}

#[rstest]
#[tokio::test]
async fn registration_accepts_structured_harness_contracts_on_native(
	validation: DefinitionValidation,
) {
	// Arrange
	let contract = "structured@3";
	let mut scope = scope_with(model_config(Some(json!(["native"]))));
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
		stored.harness_contract.unwrap_or_default(),
		serde_json::from_value(json!(contract)).unwrap()
	);
}

/// `structured@1` and `structured@2` lost the #532 comparison and were removed.
#[rstest]
#[case::structured1("structured@1")]
#[case::structured2("structured@2")]
#[tokio::test]
async fn registration_rejects_removed_harness_contracts(
	validation: DefinitionValidation,
	#[case] contract: &str,
) {
	// Arrange
	let mut scope = scope_with(model_config(Some(json!(["native"]))));
	let mut config = agent_config(Some("native"));
	config["harness_contract"] = json!(contract);
	let agent = definition("agent", "agent", config);
	// Act
	let error = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap_err();
	// Assert: the version no longer decodes.
	assert!(error.to_string().contains("unknown variant"), "{error:?}");
	assert!(!scope.entries.contains_key("agent@1.0.0"));
}

/// New Agent definitions that name neither version render Native with
/// `structured@3` when their model declares Native (#532).
#[rstest]
// model versions, agent projection, agent contract => stored projection, stored contract
#[case::native_model_defaults(Some(json!(["legacy","ordered","native"])), None, None, Some("native"), Some("structured@3"))]
#[case::native_only_model(Some(json!(["native"])), None, None, Some("native"), Some("structured@3"))]
#[case::legacy_model_keeps_omitted(Some(json!(["legacy","ordered"])), None, None, None, None)]
#[case::undeclared_model_keeps_omitted(None, None, None, None, None)]
#[case::explicit_legacy_kept(Some(json!(["legacy","native"])), Some("legacy"), None, Some("legacy"), None)]
#[case::explicit_native_gets_contract(Some(json!(["native"])), Some("native"), None, Some("native"), Some("structured@3"))]
#[case::explicit_contract_kept(Some(json!(["native"])), None, Some("structured@3"), Some("native"), Some("structured@3"))]
#[case::explicit_legacy_contract_kept(Some(json!(["native"])), Some("native"), Some("legacy@1"), Some("native"), Some("legacy@1"))]
#[tokio::test]
async fn new_agents_default_to_native_with_structured3(
	validation: DefinitionValidation,
	#[case] versions: Option<Value>,
	#[case] projection: Option<&str>,
	#[case] contract: Option<&str>,
	#[case] stored_projection: Option<&str>,
	#[case] stored_contract: Option<&str>,
) {
	// Arrange
	let mut scope = scope_with(model_config(versions));
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
	assert_eq!(
		stored.get("projection_version"),
		stored_projection.map(|v| json!(v)).as_ref()
	);
	assert_eq!(
		stored.get("harness_contract"),
		stored_contract.map(|v| json!(v)).as_ref()
	);
}

#[tokio::test]
async fn replaying_a_model_stored_without_a_cache_mode_is_idempotent() {
	// Arrange: a model registered before ADR 0115 omits the field.
	let validation = validation();
	let mut config = model_config(Some(json!(["legacy", "ordered", "native"])));
	config["model_id"] = json!("anthropic/claude-haiku-5.5");
	config["endpoint"] = json!("https://openrouter.ai/api/v1");
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
	let mut config = model_config(Some(json!(["legacy", "ordered", "native"])));
	config["model_id"] = json!(model_id);
	config["endpoint"] = json!("https://openrouter.ai/api/v1");
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
#[case::ordered_keeps_the_opt_in(Some("explicit"), Some("ordered"), None, None, None)]
#[case::legacy_contract_ends_the_prefix(
	Some("explicit"),
	Some("native"),
	Some("legacy@1"),
	None,
	None
)]
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
	let mut model = model_config(Some(json!(["legacy", "ordered", "native"])));
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

#[tokio::test]
async fn registration_rejects_structured_contracts_when_the_model_lacks_native() {
	// Arrange: no Projection Version and a model without Native keep Legacy.
	let validation = validation();
	let mut scope = scope_with(model_config(Some(json!(["legacy", "ordered"]))));
	let mut config = agent_config(None);
	config["harness_contract"] = json!("structured@3");
	let agent = definition("agent", "agent", config);
	// Act
	let error = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap_err();
	// Assert
	assert!(is_invalid(&error), "{error:?}");
	assert!(!scope.entries.contains_key("agent@1.0.0"));
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
	let mut scope = scope_with(model_config(Some(json!(["legacy", "ordered", "native"]))));
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
	let mut scope = scope_with(model_config(Some(json!(["legacy", "ordered", "native"]))));
	let agent = definition("agent", "agent", agent_config(None));
	// Act
	let returned = register(&mut scope, &validation, agent, None, true, NODE)
		.await
		.unwrap();
	// Assert
	assert_eq!(returned.config["projection_version"], json!("native"));
	assert_eq!(returned.config["harness_contract"], json!("structured@3"));
	assert_eq!(returned, scope.entries["agent@1.0.0"]);
}
