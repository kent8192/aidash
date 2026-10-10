//! Agents pin only implemented Projection Versions their model declares.
use super::*;
use aidash_domain::{projection::ProjectionVersion, registry::bindings::AgentBindings};

const NODE: &str = "aidash://test";

fn model_config(versions: Option<Value>) -> Value {
	let mut config = json!({"provider":"openrouter","model_id":"vendor/model","endpoint":"https://model.example.test","credential_env":null,"context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}});
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
	for name in aidash_domain::registry::bindings::REQUIRED_TOOLS
		.iter()
		.chain(aidash_domain::registry::bindings::DEFAULT_TOOLS)
	{
		let descriptor = aidash_domain::tool::providers::core_descriptor(NODE, name).unwrap();
		scope.put(definition(
			&format!("aidash.{name}"),
			"tool",
			serde_json::to_value(descriptor).unwrap(),
		));
	}
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
#[case::native_agent(Some(json!(["legacy","ordered"])), "native")]
#[case::native_declared_by_stored_model(Some(json!(["legacy","ordered","native"])), "native")]
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
#[case::native(json!(["legacy","native"]))]
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
fn model_definitions_accept_implemented_declarations(
	validation: DefinitionValidation,
	#[case] versions: Option<Value>,
) {
	let model = definition("model", "model", model_config(versions));
	validation.validate_in(&model, false).unwrap();
}

#[rstest]
#[tokio::test]
async fn publication_rejects_an_agent_its_model_cannot_render(validation: DefinitionValidation) {
	// Arrange
	let mut scope = scope_with(model_config(None));
	let package = Package {
		entity: definition("agent", "agent", agent_config(Some("ordered"))),
		author: "author".into(),
		permissions: Vec::new(),
		dependencies: Vec::new(),
	};
	// Act
	let error = publish(&mut scope, &validation, package).await.unwrap_err();
	// Assert
	assert!(is_invalid(&error), "{error:?}");
	assert!(!scope.trace.iter().any(|step| step == "publish"));
	assert!(scope.events.is_empty());
}

/// Serializations produced before Projection Versions existed.
const LEGACY_AGENT: &str = r#"{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"execute","bindings":[],"remove_default":[],"cluster":null,"max_steps":64}"#;
const LEGACY_MODEL: &str = r#"{"provider":"openrouter","model_id":"vendor/model","endpoint":"https://model.example.test","credential_env":null,"reasoning_effort":null,"context_window":32768,"max_output_tokens":4096,"modalities":["text"],"media_routes":[],"cost":{}}"#;

#[rstest]
fn definitions_without_projection_fields_keep_their_bytes_and_resolve_as_legacy() {
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
	let mut config = model_config(Some(json!(["legacy", "ordered"])));
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
) {
	// Arrange
	let mut scope = scope_with(cache_model("anthropic/claude-fixture", Some("explicit")));
	let mut config = agent_config(Some("ordered"));
	if let Some(cache) = prompt_cache {
		config["prompt_cache"] = json!(cache);
	}
	let agent = definition("agent", "agent", config.clone());
	// Act
	let inserted = register_definition(&mut scope, &validation, &agent, NODE)
		.await
		.unwrap();
	// Assert
	assert!(inserted);
	assert_eq!(
		scope.entries["agent@1.0.0"].config.get("prompt_cache"),
		config.get("prompt_cache")
	);
}

#[rstest]
#[case::undeclared_model("anthropic/claude-fixture", None, Some("ordered"))]
#[case::automatic_model("anthropic/claude-fixture", Some("automatic"), Some("ordered"))]
#[case::slug_outside_the_allowlist("openai/gpt-fixture", Some("explicit"), Some("ordered"))]
#[case::legacy_agent("anthropic/claude-fixture", Some("explicit"), None)]
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
#[tokio::test]
async fn publication_rejects_explicit_prompt_cache_its_model_cannot_receive(
	validation: DefinitionValidation,
) {
	// Arrange
	let mut scope = scope_with(cache_model("anthropic/claude-fixture", Some("automatic")));
	let package = Package {
		entity: definition("agent", "agent", cache_agent(Some("ordered"))),
		author: "author".into(),
		permissions: Vec::new(),
		dependencies: Vec::new(),
	};
	// Act
	let error = publish(&mut scope, &validation, package).await.unwrap_err();
	// Assert
	assert!(is_invalid(&error), "{error:?}");
	assert!(!scope.trace.iter().any(|step| step == "publish"));
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

#[rstest]
fn definitions_without_cache_fields_keep_their_bytes() {
	let model: ModelConfig = serde_json::from_str(LEGACY_MODEL).unwrap();
	let agent: AgentBindings = serde_json::from_str(LEGACY_AGENT).unwrap();
	assert_eq!(model.cache_mode, None);
	assert_eq!(agent.prompt_cache, None);
	assert_eq!(serde_json::to_string(&model).unwrap(), LEGACY_MODEL);
	assert_eq!(serde_json::to_string(&agent).unwrap(), LEGACY_AGENT);
}
