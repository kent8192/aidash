use serde_json::json;

#[rstest::rstest]
fn policy_management_routes_publish_authenticated_typed_contracts() {
	let document =
		serde_json::to_value(aidash_server::config::openapi::openapi().unwrap()).unwrap();
	for (path, method) in [
		("/api/authorization/{tenant}", "get"),
		("/api/authorization/{tenant}", "post"),
		("/api/authorization/{tenant}/evaluate", "post"),
		("/api/authorization/{tenant}/simulate", "post"),
		("/api/authorization/{tenant}/revisions", "get"),
		("/api/authorization/{tenant}/decisions", "get"),
		("/api/authorization/{tenant}/credentials", "get"),
		("/api/authorization/{tenant}/credentials", "post"),
		("/api/authorization/{tenant}/catalog", "get"),
		("/api/authorization/{tenant}/catalog", "post"),
		(
			"/api/authorization/{tenant}/credentials/{id}/revoke",
			"post",
		),
	] {
		let operation = &document["paths"][path][method];
		assert!(operation.is_object(), "missing {method} {path}");
		assert_eq!(operation["security"], json!([{"bearer_auth":[]}]));
		assert!(operation["responses"]["200"]["content"]["application/json"]["schema"].is_object());
	}
	assert!(document["components"]["schemas"]["PolicyBundle"].is_object());
	let condition = &document["components"]["schemas"]["Condition"];
	assert!(condition.is_object());
	assert!(
		condition
			.to_string()
			.contains("#/components/schemas/Condition"),
		"recursive conditions must retain their type"
	);
}
