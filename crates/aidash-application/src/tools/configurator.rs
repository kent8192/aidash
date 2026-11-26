//! Model-visible inputs for administration tools. Parameters in braces are
//! consumed by the path; the remaining fields are the route's JSON body.
use super::Builtin;
use serde_json::{Value, json};

pub struct Route {
	pub method: &'static str,
	pub path: &'static str,
	pub description: &'static str,
}

pub fn route(name: &str) -> Option<Route> {
	let (method, path, description) = match name {
		"node_update" => (
			"PATCH",
			"/api/tenants/{tenant}/nodes/{node}",
			"Change a Node label, make it the default, or update its instructions.",
		),
		"member_invite" => (
			"POST",
			"/api/tenants/{tenant}/invitations",
			"Invite a member with the proposed groups.",
		),
		"member_set_groups" => (
			"PUT",
			"/api/tenants/{tenant}/subjects/{subject}/groups",
			"Replace a member's Assignable Groups at the given policy revision.",
		),
		"registry_create" | "tool_add" | "mcp_server_add" => (
			"POST",
			"/api/registry",
			"Register an immutable definition. For a tool or MCP server supply its tool definition.",
		),
		"catalog_approve" => (
			"POST",
			"/api/authorization/{tenant}/catalog",
			"Approve a version on the selected Node or Deployment.",
		),
		"provider_binding_set" => (
			"PUT",
			"/api/tenants/{tenant}/provider-credential-bindings/{provider}",
			"Change the binding to an existing provider credential.",
		),
		"budget_set" => (
			"POST",
			"/api/tenants/{tenant}/budgets",
			"Create a Budget, or update the named Budget at its expected revision.",
		),
		"approval_mode_set" => (
			"PUT",
			"/api/threads/{thread}/approval-mode",
			"Change the person's Approval Mode for a Thread.",
		),
		"approval_rule_set" => (
			"POST",
			"/api/tenants/{tenant}/approval-rules/mine",
			"Create an Approval Rule for the requester.",
		),
		"tenant_instructions_set" => (
			"PUT",
			"/api/tenants/{tenant}/instructions",
			"Replace the Tenant Instructions.",
		),
		"data_protection_set" => (
			"PUT",
			"/api/tenants/{tenant}/data-protection",
			"Set the Tenant data protection policy.",
		),
		"tenant_secret_set" => (
			"PUT",
			"/api/tenants/{tenant}/secrets/{name}",
			"Change the hosts for an existing Tenant Secret. Use setting_request for its value.",
		),
		"stint_start" => (
			"POST",
			"/api/threads/{thread}/agents/{agent}/executions",
			"Start an agent in a Thread.",
		),
		"marketplace_install" => (
			"POST",
			"/api/marketplace/packages/{key}/install",
			"Install the selected Marketplace release.",
		),
		"reference_upload" => (
			"POST",
			"/api/references/batch",
			"Upload Reference files with paths and base64 data.",
		),
		"duty_template_install" => (
			"POST",
			"/api/marketplace/packages/{key}/duty",
			"Install a Duty from a Marketplace template.",
		),
		"generation_policy_set" => (
			"POST",
			"/api/generation/{tenant}/policies/{id}",
			"Set a Generation Policy.",
		),
		"node_registration" => (
			"POST",
			"/api/tenants/{tenant}/nodes/{node}/registrations",
			"Register another Node, or update its Peer Mapping.",
		),
		"agent_create" | "agent_plugin_add" | "skill_add" => (
			"POST",
			"/api/workbench/drafts",
			"Create, register and approve the supplied agent version, including its attached plugins and Skills.",
		),
		_ => return None,
	};
	Some(Route {
		method,
		path,
		description,
	})
}

fn inputs(name: &str) -> Value {
	let string = json!({"type":"string"});
	let integer = json!({"type":"integer"});
	let strings = json!({"type":"array","items":string});
	let object = json!({"type":"object"});
	let mut schemas = schemars::SchemaGenerator::default();
	let approval = || {
		json!({
			"type":"object",
			"properties":{"node":string,"scope":{"enum":["node","deployment"]},
				"expected_revision":integer,"expected_node_revisions":{"type":"object","additionalProperties":integer},
				"carry_over":{"type":"boolean"},"provider_warning_acknowledged":{"type":"boolean"},
				"groups":strings,"restriction":{"type":"object","properties":{"groups":strings,"subjects":strings}}},
			"required":["node","scope","expected_revision"],
			"additionalProperties":false
		})
	};
	let (fields, required) = match name {
		"aidash_api" => (
			json!({"method":{"enum":["GET","POST","PUT","PATCH","DELETE"]},"path":string,"body":{}}),
			vec!["method", "path"],
		),
		"aidash_api_catalog" => (json!({"query":string}), vec!["query"]),
		"tenant_overview" => (json!({}), vec![]),
		"node_update" => (
			json!({"node":string,"action":{"enum":["label","default","instructions"]},"label":string,"text":string,"expected_revision":integer}),
			vec!["node", "action"],
		),
		"member_invite" => (
			json!({"groups":strings,"expires_in_seconds":integer,"join_directly":{"type":"boolean"}}),
			vec![],
		),
		"member_set_groups" => (
			json!({"subject":string,"groups":strings,"expected_policy_revision":integer}),
			vec!["subject", "groups", "expected_policy_revision"],
		),
		"data_protection_set" => {
			return json!(schemars::schema_for!(
				aidash_domain::data_protection::DataProtection
			));
		}
		"stint_start" => (
			json!({"agent":string,"thread":string,"idempotency_key":string,"agent_version":string,"title":string,"description":string,"task_model":object,"read_pages":strings}),
			vec![
				"agent",
				"thread",
				"idempotency_key",
				"agent_version",
				"title",
				"description",
			],
		),
		"marketplace_install" => (
			json!({"key":string,"digest":string,"config":object,"bindings":{"type":"array","items":object},"secrets":object,"idempotency_key":string}),
			vec!["key", "digest", "idempotency_key"],
		),
		"duty_template_install" => (
			json!({"key":string,"digest":string,"thread_id":string,"agent":object,"idempotency_key":string}),
			vec!["key", "digest", "thread_id", "idempotency_key"],
		),
		"registry_create" | "tool_add" | "mcp_server_add" => {
			return json!(schemars::schema_for!(aidash_domain::registry::Entry));
		}
		"catalog_approve" => (
			json!({"node":string,"entry":{"type":"object","properties":{"id":string,"version":string},"required":["id","version"]},"expected_revision":integer,"scope":{"enum":["node","deployment"]},"enabled":{"type":"boolean"},"groups":strings,"restriction":object,"expected_node_revisions":object,"carry_over":{"type":"boolean"},"provider_warning_acknowledged":{"type":"boolean"}}),
			vec!["node", "entry", "expected_revision", "scope"],
		),
		"provider_binding_set" => (
			json!({"provider":string,"provider_credential_id":{"type":["null","string"]},"expected_revision":integer}),
			vec!["provider", "provider_credential_id", "expected_revision"],
		),
		"budget_set" => (
			json!({"budget":string,"definition":schemas.subschema_for::<aidash_domain::budgets::Definition>(),"expected_revision":integer}),
			vec!["definition"],
		),
		"approval_mode_set" => (
			json!({"thread":string,"mode":{"type":["null","string"]}}),
			vec!["thread", "mode"],
		),
		"approval_rule_set" => (
			json!({"kind":string,"effect":string,"target":{}}),
			vec!["kind", "effect", "target"],
		),
		"tenant_instructions_set" => (
			json!({"expected_revision":integer,"text":string}),
			vec!["expected_revision", "text"],
		),
		"tenant_secret_set" => (
			json!({"name":string,"expected_revision":integer,"hosts":strings}),
			vec!["name", "expected_revision", "hosts"],
		),
		"reference_upload" => (
			json!({"thread":string,"filename":string,"media_type":string,"idempotency_key":string,"data":string,"files":{"type":"array","items":{"type":"object","properties":{"data":string,"path":string},"required":["data","path"],"additionalProperties":false}}}),
			vec![],
		),
		"agent_create" => (
			json!({"draft":schemas.subschema_for::<aidash_domain::registry::workbench::CreateDraft>(),"approval":approval()}),
			vec!["draft", "approval"],
		),
		"skill_add" => (
			json!({"draft":schemas.subschema_for::<aidash_domain::registry::workbench::CreateDraft>(),"approval":approval(),"skill":schemas.subschema_for::<aidash_domain::registry::Entry>(),"skill_approval":approval(),"source":{"type":"object","properties":{"url":string,"skill_path":string},"required":["url"]}}),
			vec!["draft", "approval", "skill", "skill_approval"],
		),
		"agent_plugin_add" => (
			json!({"draft":schemas.subschema_for::<aidash_domain::registry::workbench::CreateDraft>(),"approval":approval(),"plugin":schemas.subschema_for::<aidash_domain::agent_plugin::AgentPluginRef>(),"source":{"type":"object","properties":{"url":string,"plugin_path":string},"required":["url"]}}),
			vec!["draft", "approval"],
		),
		"node_registration" => (
			json!({"node":string,"registration":schemas.subschema_for::<aidash_domain::nodes::registration::RegistrationInput>(),"peer_mapping":schemas.subschema_for::<aidash_domain::identity::peer_mapping::PeerMappingInput>()}),
			vec!["node"],
		),
		"generation_policy_set" => (
			json!({"id":string,"expected_revision":integer,"spec":schemas.subschema_for::<aidash_domain::generation::policy::Spec>()}),
			vec!["id", "expected_revision", "spec"],
		),
		_ => unreachable!("undeclared Configurator operation"),
	};
	let mut input = json!({"type":"object","properties":fields,"required":required,"additionalProperties":false});
	let definitions = schemas.take_definitions(true);
	if !definitions.is_empty() {
		input["$defs"] = Value::Object(definitions);
	}
	input
}

pub(super) fn builtins() -> impl Iterator<Item = Builtin> {
	aidash_domain::tool::configurator::TOOLS.iter().map(|&name|Builtin{name,
		contract:aidash_domain::tool::builtin_contract(name).expect("declared configurator tool"),
		description:match name {
			"aidash_api"=>"Call an Aidash JSON API in this Tenant as the requester. Pass a relative API path.",
			"aidash_api_catalog"=>"Find permitted API operations by path or operation name, with their parameter and body schemas.",
			"tenant_overview"=>"Read Nodes, memberships, approved definitions, provider connections, Budgets and approval preferences.",
			_=>route(name).expect("configurator route").description,
		},schema:inputs(name)})
}
