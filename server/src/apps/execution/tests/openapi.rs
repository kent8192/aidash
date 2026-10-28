use crate::config::openapi::openapi;
use rstest::{fixture, rstest};
use serde_json::{Value, json, to_value};
use std::collections::BTreeSet;

#[fixture]
fn document() -> Value {
	to_value(openapi().expect("valid endpoint contracts")).expect("serializable document")
}

#[rstest]
fn openapi_describes_authenticated_management_routes_and_streams(document: Value) {
	assert!(document["components"]["schemas"]["Policy"]["properties"]["actions"].is_object());
	assert!(
		document["components"]["schemas"]["GenerationPolicy"]["properties"]["spec"].is_object()
	);
	let paths = document["paths"].as_object().unwrap();
	for (path, method) in [
		("/api/marketplace/packages", "get"),
		("/api/marketplace/packages/{key}", "get"),
		("/api/marketplace/packages", "post"),
		("/api/marketplace/sources", "get"),
		("/api/marketplace/packages/{key}/install", "post"),
		("/api/marketplace/packages/{key}/audience", "put"),
		("/api/marketplace/packages/{key}/consents/{tenant}", "put"),
		("/api/marketplace/packages/{key}/consents/{tenant}", "get"),
		("/api/marketplace/installations", "get"),
		("/api/marketplace/installations/{id}", "get"),
		("/api/marketplace/installations/{id}", "post"),
		("/api/marketplace/installations/{id}/activation", "post"),
		("/api/marketplace/administration", "get"),
		("/api/marketplace/publication-access", "post"),
		("/api/workspaces/{id}/threads", "post"),
		("/api/workspaces/{id}/thread-messages", "post"),
		("/api/workspaces/{id}/message-history", "get"),
		("/api/workspaces/{id}/attachments", "post"),
		("/api/workspaces/{id}/attachments/{attachment_id}", "get"),
		("/api/providers/openrouter/models", "get"),
		("/api/agents/personal", "post"),
		("/api/workbench/drafts", "post"),
		("/api/workbench/drafts/{id}", "put"),
		("/api/workbench/drafts/{id}/register", "post"),
		("/api/workbench/drafts/{id}/tests", "post"),
		("/api/workbench/versions/{id}/{version}", "get"),
		("/api/workbench/versions/{id}/{version}/report", "get"),
		("/api/registry/mcp-tool-hint", "post"),
		("/api/skills/import", "post"),
		("/api/agent-plugins/import", "post"),
		("/api/tasks", "get"),
		("/api/tasks/{id}/remote-grants", "post"),
		("/api/tasks/{id}/remote-grants/{grant}/revoke", "post"),
		("/api/authorization/{tenant}/peer-mappings", "get"),
		("/api/authorization/{tenant}/peer-mappings", "post"),
		("/api/authorization/{tenant}/peer-mapping-history", "get"),
		("/api/tasks/{task}/remote-executions", "get"),
		("/api/tasks/{task}/remote-grants/{id}/messages", "post"),
	] {
		assert!(document["paths"][path][method].is_object());
	}
	let mut operation_ids = BTreeSet::new();
	for (path, operations) in paths {
		// Public browser endpoints that publish a typed contract: desktop
		// sign-in, and an Invitation holder opening or redeeming a link.
		let public = path.starts_with("/auth/desktop/") || path.starts_with("/auth/invitations/");
		assert!(
			path.starts_with("/api/") || path.starts_with("/federation/v0.1/") || public,
			"{path}"
		);
		for operation in operations.as_object().unwrap().values() {
			let id = operation["operationId"].as_str().expect("named operation");
			assert!(operation_ids.insert(id), "duplicate operation {id}");
			if public {
				assert_eq!(operation["security"], json!([{}]), "{path}");
			} else {
				assert_eq!(operation["security"][0]["bearer_auth"], json!([]));
			}
		}
	}
	assert!(document["paths"]["/api/events/stream"]["get"]["responses"]["200"]["content"]["text/event-stream"].is_object());
	assert!(document["components"]["schemas"]["StateResponse"]["properties"]["tasks"].is_object());
	assert_eq!(
		document["components"]["schemas"]["Request_Search"]["additionalProperties"],
		false
	);
}

#[rstest]
fn openapi_distinguishes_input_defaults_from_serialized_responses(document: Value) {
	let schemas = &document["components"]["schemas"];
	// Control returns the same restricted inspection as Run reads. Advertising
	// the stored Run would publish source payloads and private execution state
	// in the generated client contract even though HTTP serialization omits them.
	assert_eq!(
		document["paths"]["/api/runs/{id}/control"]["post"]["responses"]["200"]["content"]["application/json"]
			["schema"],
		json!({"$ref":"#/components/schemas/RunInspection"})
	);
	for field in [
		"source_contributions",
		"source_selections",
		"source_observation",
		"source_deliveries",
	] {
		assert!(
			schemas["InspectionContext"]["properties"]
				.get(field)
				.is_none(),
			"private source execution field {field} is not a public API contract"
		);
	}
	assert!(schemas["GenerationRequest"]["properties"].is_object());
	let output_required = schemas["Entry"]["required"].as_array().unwrap();
	let input_required = schemas["Request_Entry"]["required"].as_array().unwrap();
	for field in ["config", "capabilities", "languages", "skills"] {
		assert!(
			output_required.contains(&json!(field)),
			"serialized field {field} is always present"
		);
		assert!(
			!input_required.contains(&json!(field)),
			"input field {field} has a serde default"
		);
	}
	assert!(
		schemas["GenerationRequest"]["properties"]
			.get("credential_id")
			.is_none()
	);
	assert_eq!(
		schemas["AtomicTransaction"]["properties"]["manifest"]["$ref"],
		"#/components/schemas/TransactionManifest"
	);
	assert!(schemas.get("PackageRecord").is_none());
	let upload = &document["paths"]["/api/workspaces/{id}/attachments"]["post"];
	assert_eq!(
		upload["requestBody"]["content"]["application/octet-stream"]["schema"]["format"],
		"binary"
	);
	let parameters =
		document["paths"]["/api/authorization/{tenant}/credentials"]["get"]["parameters"]
			.as_array()
			.unwrap();
	assert!(
		parameters
			.iter()
			.any(|parameter| parameter["name"] == "offset" && parameter["in"] == "query")
	);
	for (path, method, schema) in [
		(
			"/api/marketplace/packages/{key}/install",
			"post",
			"MarketplaceInstallationRevision",
		),
		(
			"/api/marketplace/installations/{id}",
			"post",
			"MarketplaceInstallationRevision",
		),
		(
			"/api/marketplace/publication-access",
			"post",
			"MarketplacePublicationPreview",
		),
	] {
		assert_eq!(
			document["paths"][path][method]["responses"]["200"]["content"]["application/json"]["schema"]
				["$ref"],
			format!("#/components/schemas/{schema}"),
			"Marketplace responses preserve the dashboard contract at {path}"
		);
	}
	for path in [
		"/api/marketplace/sources",
		"/api/marketplace/administration",
	] {
		assert_eq!(
			document["paths"][path]["get"]["responses"]["200"]["headers"]["x-aidash-next-offset"]["schema"]
				["type"],
			"integer"
		);
	}
	assert_references_resolve(&document, &document);
}

#[rstest]
#[case("/api/runs/{id}/semantic", "get", "remote-semantic-run-provenance")]
#[case(
	"/api/tasks/{task}/remote-grants/{id}/semantic",
	"get",
	"remote-semantic-home-provenance"
)]
#[case(
	"/api/tasks/{task}/remote-grants/{id}/follow-up",
	"post",
	"remote-execution-follow-up"
)]
#[case(
	"/api/tasks/{id}/remote-generation",
	"post",
	"remote-generation-prepare"
)]
#[case(
	"/api/tasks/{id}/remote-generation/{intent}/cancel",
	"post",
	"remote-generation-cancel"
)]
fn openapi_retains_current_management_endpoints(
	document: Value,
	#[case] path: &str,
	#[case] method: &str,
	#[case] operation_id: &str,
) {
	let operation = &document["paths"][path][method];
	assert_eq!(operation["operationId"], operation_id);
	assert!(operation["responses"]["200"].is_object());
	let parameters = operation["parameters"].as_array().expect("path parameters");
	assert_eq!(parameters.len(), path.matches('{').count());
	assert!(parameters.iter().all(|parameter| parameter["in"] == "path"
		&& parameter["required"] == true
		&& parameter["schema"]["format"] == "uuid"));
}

/// One route serves a Tenant Subject and a Deployment Administrator's Safety
/// Stop (ADR 0101), so each side's contract is a separate schema.
#[rstest]
fn openapi_separates_the_subject_and_safety_stop_management_contracts(document: Value) {
	let schemas = &document["components"]["schemas"];
	let get = &document["paths"]["/api/runs/{id}/management"]["get"];
	let post = &document["paths"]["/api/runs/{id}/management"]["post"];
	assert_eq!(get["operationId"], "run-management-get");
	assert_eq!(post["operationId"], "run-management-control");
	let named = |operation: &Value, location: &str| -> Vec<String> {
		operation["parameters"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|parameter| parameter["in"] == location)
			.map(|parameter| parameter["name"].as_str().unwrap().to_owned())
			.collect()
	};
	assert_eq!(named(get, "path"), ["id"]);
	assert_eq!(named(get, "query"), ["tenant"]);
	assert_eq!(named(post, "path"), ["id"]);
	assert_eq!(named(post, "header"), ["Idempotency-Key"]);
	// An administrator's metadata is its own projection, never RunManagement.
	let facts = schemas["RunControlFacts"]["properties"]
		.as_object()
		.unwrap();
	let mut fields = facts.keys().map(String::as_str).collect::<Vec<_>>();
	fields.sort_unstable();
	assert_eq!(
		fields,
		[
			"control",
			"control_revision",
			"control_source",
			"id",
			"phase"
		]
	);
	let receipt = schemas["SafetyStopReceipt"]["properties"]
		.as_object()
		.unwrap();
	let mut fields = receipt.keys().map(String::as_str).collect::<Vec<_>>();
	fields.sort_unstable();
	assert_eq!(
		fields,
		["action", "operation_id", "outcome", "run_id", "tenant"]
	);
	assert_eq!(schemas["SafetyStopInput"]["additionalProperties"], false);
	assert!(
		schemas["SafetyStopInput"]["required"]
			.as_array()
			.unwrap()
			.iter()
			.any(|name| name == "reason")
	);
	assert!(document["paths"]["/api/authorization/{tenant}/safety-stops"]["get"].is_object());
}

/// The Node a read is made for and the paging of the Node Registration reads
/// are part of the contract, so the generated web client sends them.
#[rstest]
#[case("/api/discover", "post", &[("node", false)])]
#[case("/api/federation/graph/peers", "get", &[("node", false)])]
#[case("/api/authorization/{tenant}/catalog", "get", &[("node", true)])]
#[case("/api/registry/versions", "get", &[("node", true), ("offset", false)])]
#[case(
	"/api/tenants/{tenant}/nodes/{node}/registrations",
	"get",
	&[("offset", false), ("limit", false)]
)]
#[case(
	"/api/tenants/{tenant}/nodes/{node}/registration-history",
	"get",
	&[("after", false), ("limit", false)]
)]
fn openapi_declares_the_node_and_paging_queries(
	document: Value,
	#[case] path: &str,
	#[case] method: &str,
	#[case] queries: &[(&str, bool)],
) {
	let parameters = document["paths"][path][method]["parameters"]
		.as_array()
		.expect("parameters");
	for (name, required) in queries {
		let parameter = parameters
			.iter()
			.find(|parameter| parameter["name"] == *name && parameter["in"] == "query")
			.unwrap_or_else(|| panic!("{method} {path} declares the query {name}"));
		assert_eq!(parameter["required"], *required, "{method} {path} {name}");
	}
}

#[rstest]
fn the_state_names_its_deployment_and_no_node(document: Value) {
	let state = &document["components"]["schemas"]["StateResponse"];
	assert_eq!(
		state["properties"]["deployment"]["$ref"],
		"#/components/schemas/DeploymentIdentity"
	);
	assert!(state["properties"].get("node").is_none());
	assert!(
		document["components"]["schemas"]
			.get("NodeIdentity")
			.is_none()
	);
}

#[rstest]
fn openapi_retains_dashboard_component_names(document: Value) {
	let schemas = &document["components"]["schemas"];
	for name in [
		"ReferenceDocument",
		"Evaluation",
		"RemoteSemanticStatus",
		"RemoteSemanticProvenance",
		"RemoteSemanticFailure",
		"RunManagement",
		"GenerationAction",
		"GenerationRequest",
		"GenerationSpec",
		"GenerationRemoteApprovals",
		"PeerMappingInput",
		"RemoteGenerationInput",
		"RemoteGenerationPrepared",
		"SemanticIndexSpec",
		"SemanticSource",
	] {
		assert!(
			schemas[name].is_object(),
			"missing dashboard component {name}"
		);
	}
	assert_eq!(schemas["BTreeMap"]["type"], "object");
	assert_eq!(
		schemas["BTreeMap"]["additionalProperties"]["type"],
		"string"
	);
	for name in [
		"ReferenceDocument",
		"Evaluation",
		"GenerationAction",
		"PeerMappingInput",
		"RemoteGenerationInput",
		"SemanticIndexSpec",
		"SemanticSource",
	] {
		assert_eq!(
			schemas[name],
			schemas[format!("Request_{name}")],
			"input-only aliases preserve defaults for {name}"
		);
	}
	assert!(
		!schemas["GenerationRequest"]["properties"]
			.as_object()
			.unwrap()
			.contains_key("foreign_intent")
	);
	assert_references_resolve(&document, &document);
}

fn assert_references_resolve(value: &Value, document: &Value) {
	match value {
		Value::Object(fields) => {
			if let Some(Value::String(reference)) = fields.get("$ref")
				&& let Some(pointer) = reference.strip_prefix('#')
			{
				assert!(
					document.pointer(pointer).is_some(),
					"unresolved schema {reference}"
				);
			}
			for child in fields.values() {
				assert_references_resolve(child, document);
			}
		}
		Value::Array(values) => {
			for child in values {
				assert_references_resolve(child, document);
			}
		}
		_ => {}
	}
}
