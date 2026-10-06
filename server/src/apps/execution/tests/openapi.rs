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
		("/api/marketplace/compatibility", "get"),
		("/api/marketplace/compatibility", "put"),
		("/api/marketplace/installations/{id}/activation", "post"),
		("/api/marketplace/adoptions", "post"),
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
		("/api/skills/import", "post"),
		("/api/tasks", "get"),
		("/api/tasks/{id}/remote-grants", "post"),
		("/api/tasks/{id}/remote-grants/{grant}/revoke", "post"),
		("/api/authorization/{tenant}/peer-mappings", "get"),
		("/api/authorization/{tenant}/peer-mappings", "post"),
		("/api/authorization/{tenant}/peer-mapping-history", "get"),
		("/api/tasks/{task}/remote-executions", "get"),
		("/api/tasks/{task}/remote-grants/{id}/messages", "post"),
		("/api/transactions/trust", "get"),
	] {
		assert!(document["paths"][path][method].is_object());
	}
	let mut operation_ids = BTreeSet::new();
	for (path, operations) in paths {
		assert!(
			path.starts_with("/api/")
				|| path.starts_with("/federation/v0.1/")
				|| path.starts_with("/auth/desktop/")
		);
		for operation in operations.as_object().unwrap().values() {
			let id = operation["operationId"].as_str().expect("named operation");
			assert!(operation_ids.insert(id), "duplicate operation {id}");
			if path.starts_with("/auth/desktop/") {
				assert_eq!(operation["security"], json!([{}]));
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
	assert_eq!(
		schemas["PackageRecord"]["properties"]["manifest"]["$ref"],
		"#/components/schemas/Package"
	);
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
		(
			"/api/marketplace/compatibility",
			"put",
			"MarketplaceCompatibility",
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
#[case("/api/runs/{id}/management", "get", "run-management-get")]
#[case("/api/runs/{id}/management", "post", "run-management-control")]
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
