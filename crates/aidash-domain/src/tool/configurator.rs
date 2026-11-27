//! Tools reserved for the Tenant Configurator and the local API surface it can call.
pub const TOOLS: &[&str] = &[
	"aidash_api",
	"aidash_api_catalog",
	"tenant_overview",
	"node_update",
	"member_invite",
	"member_set_groups",
	"registry_create",
	"catalog_approve",
	"catalog_review_approve",
	"provider_binding_set",
	"budget_set",
	"approval_mode_set",
	"approval_rule_set",
	"tenant_instructions_set",
	"data_protection_set",
	"tenant_secret_set",
	"stint_start",
	"marketplace_install",
	"mcp_server_add",
	"tool_add",
	"agent_plugin_add",
	"skill_add",
	"reference_upload",
	"duty_template_install",
	"agent_create",
	"generation_policy_set",
	"node_registration",
];

/// Match literal path segments before the router decodes parameters. Every
/// accepted path has one interpretation. Query strings may carry read filters.
pub fn permits(tenant: &str, method: &str, uri: &str) -> bool {
	if !matches!(method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE")
		|| !uri.is_ascii()
		|| uri
			.bytes()
			.any(|b| b.is_ascii_control() || matches!(b, b'#' | b'\\' | b' '))
	{
		return false;
	}
	let (path, _) = uri.split_once('?').unwrap_or((uri, ""));
	let Some(path) = path.strip_prefix("/api/") else {
		return false;
	};
	let parts: Vec<_> = path.split('/').collect();
	if parts.iter().any(|p| {
		p.is_empty()
			|| matches!(*p, "." | "..")
			|| !p
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b"-._@".contains(&b))
	}) {
		return false;
	}
	// Personal consent, credential issuance and destructive lifecycle operations
	// need the person. Never relay a bearer token or a human answer to the model.
	if parts.iter().any(|p| {
		matches!(
			*p,
			"purge"
				| "deletion" | "credentials"
				| "registration-keys"
				| "authorizations"
				| "provider-authorizations"
				| "human-requests"
				| "setting-requests"
				| "provider-credentials"
				| "leave"
		)
	}) {
		return false;
	}
	match parts.as_slice() {
		["tenants" | "authorization" | "generation", owner, ..] => *owner == tenant,
		["workbench", "test-limits" | "test-profiles", owner, ..] => *owner == tenant,
		["marketplace", "packages", _, "consents", owner] => *owner == tenant,
		[
			"registry" | "marketplace" | "duties" | "workbench" | "references" | "agents"
			| "threads" | "tasks",
			..,
		] => true,
		_ => false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn configurator_routes_stay_within_the_requesters_tenant() {
		for path in [
			"/api/tenants/mine/nodes",
			"/api/tenants/mine/oauth-clients",
			"/api/registry",
			"/api/authorization/mine/catalog?node=a",
			"/api/workbench/test-profiles/mine",
		] {
			assert!(permits("mine", "GET", path), "{path}");
		}
		for path in [
			"/api/tenants/other/nodes",
			"/api/dashboard/foo",
			"/api/setting-requests/123/answer",
			"/api/registry/../dashboard",
			"/api/tenants/mine/purge",
			"/api/tenants/mine/nodes/a/purge",
			"/api/tenants/mine/nodes/a/deletion",
			"/api/authorization/mine/credentials",
			"/api/threads/%2e%2e/dashboard",
			"/api/workbench/test-profiles/other",
			"/api/marketplace/packages/x/consents/other",
			"https://elsewhere/api/tasks",
		] {
			assert!(!permits("mine", "POST", path), "{path}");
		}
	}
}
