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
