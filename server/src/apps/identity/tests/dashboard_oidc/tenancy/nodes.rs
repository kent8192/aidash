//! A Tenant's Nodes: the default Node every Tenant starts with, "Nodes per
//! Tenant" and a Tier's limit, and who may list, create and relabel them
//! (#241), over HTTP and PostgreSQL.
use super::*;
use aidash_application::ports::nodes::TierLimits;

/// A Tier that allows one Node, like the Free Tier #242 describes.
struct OneNode;

#[async_trait::async_trait]
impl TierLimits for OneNode {
	async fn nodes(
		&self,
		_: &aidash_domain::tenancy::TenantName,
	) -> aidash_application::Result<Option<u32>> {
		Ok(Some(1))
	}
}

impl Deployment {
	pub(super) async fn node_tenant(&self, person: &Person, label: &str) -> (String, String) {
		let (status, created) = self
			.as_person(person, "POST", "/api/tenants", None, json!({"label":label}))
			.await;
		assert_eq!(status, 201, "{created}");
		let tenant = created["name"].as_str().unwrap().to_owned();
		let context = self.context(person, &tenant).await;
		(tenant, context)
	}

	pub(super) async fn nodes_of(
		&self,
		person: &Person,
		context: &str,
		tenant: &str,
	) -> Vec<Value> {
		let (status, nodes) = self
			.as_person(
				person,
				"GET",
				&format!("/api/tenants/{tenant}/nodes"),
				Some(context),
				Value::Null,
			)
			.await;
		assert_eq!(status, 200, "{nodes}");
		nodes.as_array().unwrap().clone()
	}

	pub(super) async fn create_node(
		&self,
		person: &Person,
		context: &str,
		tenant: &str,
		label: &str,
	) -> (u16, Value) {
		self.as_person(
			person,
			"POST",
			&format!("/api/tenants/{tenant}/nodes"),
			Some(context),
			json!({"label":label}),
		)
		.await
	}
}

#[rstest::rstest]
#[tokio::test]
async fn every_way_to_create_a_tenant_leaves_one_default_node(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(
		runtime,
		TenancyRules {
			personal_tenants: true,
			..open_rules()
		},
	)
	.await;
	let alice = d.person("alice").await;
	let (status, created) = d
		.as_deployment_administrator("POST", "/api/tenants", json!({"label":"Research"}))
		.await;
	assert_eq!(status, 201, "{created}");
	let (by_person, context) = d.node_tenant(&alice, "Team").await;
	let (status, session) = d
		.as_person(&alice, "GET", "/auth/session", None, Value::Null)
		.await;
	assert_eq!(status, 200, "{session}");
	assert!(session["personal_account"].as_bool().unwrap(), "{session}");

	let tenants = d.count("SELECT count(*) FROM tenants").await;
	assert_eq!(tenants, 3);
	assert_eq!(
		d.count("SELECT count(*) FROM nodes WHERE is_default").await,
		tenants
	);
	assert_eq!(d.count("SELECT count(*) FROM nodes").await, tenants);
	assert_eq!(d.count("SELECT count(*) FROM node_names").await, tenants);
	let nodes = d.nodes_of(&alice, &context, &by_person).await;
	assert_eq!(nodes.len(), 1, "{nodes:?}");
	assert_eq!(nodes[0]["default"], true);
	assert_eq!(nodes[0]["label"], "Default");
	let name = nodes[0]["name"].as_str().unwrap();
	assert_assigned_name(name);
	assert_eq!(
		nodes[0]["address"],
		format!("{}/{name}", d.federation.config.deployment)
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn nodes_per_tenant_refuses_a_creation_beyond_the_rule(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;

	let labels: Vec<_> = (1..=12).map(|index| format!("Node {index}")).collect();
	let created = join_all(
		labels
			.iter()
			.map(|label| d.create_node(&alice, &context, &tenant, label)),
	)
	.await;

	let accepted = created.iter().filter(|(status, _)| *status == 201).count();
	assert_eq!(
		accepted, 9,
		"the default Node plus nine reach ten: {created:?}"
	);
	assert!(
		created
			.iter()
			.filter(|(status, _)| *status != 201)
			.all(|(status, body)| *status == 403 && body["error"] == "node_limit_reached"),
		"{created:?}"
	);
	assert_eq!(d.nodes_of(&alice, &context, &tenant).await.len(), 10);
	let (status, refused) = d.create_node(&alice, &context, &tenant, "Eleventh").await;
	assert_eq!(status, 403, "{refused}");
	assert_eq!(refused["error"], "node_limit_reached");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_rule_can_be_lowered_and_a_tier_may_be_stricter(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_tier_limits(
		runtime,
		TenancyRules {
			nodes_per_tenant: 5,
			..open_rules()
		},
		Some(Arc::new(OneNode)),
	)
	.await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;

	let (status, refused) = d.create_node(&alice, &context, &tenant, "Second").await;

	assert_eq!(status, 403, "{refused}");
	assert_eq!(refused["error"], "node_limit_reached");
	assert_eq!(d.nodes_of(&alice, &context, &tenant).await.len(), 1);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn members_list_nodes_and_administrators_create_and_relabel_them(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let bob_context = d.admit(&tenant, &bob, "bob", json!(["members"])).await;

	let (status, created) = d.create_node(&alice, &context, &tenant, "  Build  ").await;
	assert_eq!(status, 201, "{created}");
	assert_eq!(created["label"], "Build");
	assert_eq!(created["default"], false);
	assert_assigned_name(created["name"].as_str().unwrap());
	let node = created["name"].as_str().unwrap().to_owned();

	assert_eq!(d.nodes_of(&bob, &bob_context, &tenant).await.len(), 2);
	let (status, body) = d.create_node(&bob, &bob_context, &tenant, "Mine").await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = d
		.as_person(
			&bob,
			"PATCH",
			&format!("/api/tenants/{tenant}/nodes/{node}"),
			Some(&bob_context),
			json!({"label":"Renamed"}),
		)
		.await;
	assert_eq!(status, 403, "{body}");

	let (status, body) = d
		.as_person(
			&alice,
			"PATCH",
			&format!("/api/tenants/{tenant}/nodes/{node}"),
			Some(&context),
			json!({"label":"CI"}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["label"], "CI");
	assert_eq!(body["name"], node);
	// A relabel changes policy attributes, so each accepted actor is recorded.
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND action = 'node.update' AND actor_kind = 'tenant_administrator'")).await, 1);
	let (status, body) = d
		.as_deployment_administrator(
			"PATCH",
			&format!("/api/tenants/{tenant}/nodes/{node}"),
			json!({"label":"Deployment CI"}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND action = 'node.update' AND actor_kind = 'deployment_administrator'")).await, 1);
	for label in ["", "   ", "line\nbreak"] {
		let (status, _) = d.create_node(&alice, &context, &tenant, label).await;
		assert_eq!(status, 400, "{label:?}");
	}
	let (status, body) = d
		.as_person(
			&alice,
			"PATCH",
			&format!("/api/tenants/{tenant}/nodes/unknown-node-00000000"),
			Some(&context),
			json!({"label":"CI"}),
		)
		.await;
	assert_eq!(status, 404, "{body}");
	let (status, administration) = d
		.as_person(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/administration"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{administration}");
	let actions = administration["actions"].as_array().unwrap();
	for action in ["node.create", "node.update"] {
		assert!(
			actions.iter().any(|held| held == action),
			"{administration}"
		);
	}
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_cannot_reach_another_tenants_nodes(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (alice_tenant, alice_context) = d.node_tenant(&alice, "Alice").await;
	let (_, bob_context) = d.node_tenant(&bob, "Bob").await;

	let (status, body) = d
		.as_person(
			&bob,
			"GET",
			&format!("/api/tenants/{alice_tenant}/nodes"),
			Some(&bob_context),
			Value::Null,
		)
		.await;

	assert!(matches!(status, 403 | 404), "{status} {body}");
	let own = d.nodes_of(&alice, &alice_context, &alice_tenant).await;
	assert_eq!(own.len(), 1);
	d.finish().await;
}

impl Deployment {
	async fn workspace_on(
		&self,
		person: &Person,
		context: &str,
		node: Option<&str>,
	) -> (u16, Value) {
		let mut body = json!({"title":"Work","goal":"Use a Node"});
		if let Some(node) = node {
			body["node"] = json!(node);
		}
		self.as_person(person, "POST", "/api/workspaces", Some(context), body)
			.await
	}

	/// Replaces `members`' policy so it no longer allows `node.use`.
	async fn withdraw_node_use(&self, tenant: &str) {
		let path = format!("/api/authorization/{tenant}");
		let (status, snapshot) = self
			.as_deployment_administrator("GET", &path, Value::Null)
			.await;
		assert_eq!(status, 200, "{snapshot}");
		let mut bundle = snapshot["bundle"].clone();
		for policy in bundle["policies"].as_array_mut().unwrap() {
			if let Some(actions) = policy["actions"].as_array_mut() {
				actions.retain(|action| action != "node.use");
			}
		}
		let (status, replaced) = self
			.as_deployment_administrator(
				"POST",
				&path,
				json!({"expected_revision":snapshot["revision"],"bundle":bundle}),
			)
			.await;
		assert_eq!(status, 200, "{replaced}");
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_workspace_goes_to_the_node_its_creator_chooses_and_may_use(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let carol = d.person("carol").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let bob_context = d.admit(&tenant, &bob, "bob", json!(["members"])).await;
	let (other, other_context) = d.node_tenant(&carol, "Other").await;
	let default = d.nodes_of(&alice, &context, &tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let (_, second) = d.create_node(&alice, &context, &tenant, "Build").await;
	let second = second["name"].as_str().unwrap().to_owned();
	let foreign = d.nodes_of(&carol, &other_context, &other).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();

	let (status, workspace) = d.workspace_on(&bob, &bob_context, None).await;
	assert_eq!(status, 200, "{workspace}");
	assert_eq!(workspace["node"], default);
	let (status, workspace) = d.workspace_on(&bob, &bob_context, Some(&second)).await;
	assert_eq!(status, 200, "{workspace}");
	assert_eq!(workspace["node"], second);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM workspaces WHERE tenant = '{tenant}' AND node = '{second}'"
		))
		.await,
		1
	);
	let (status, body) = d.workspace_on(&bob, &bob_context, Some(&foreign)).await;
	assert_eq!(status, 404, "another Tenant's Node is unknown: {body}");
	let (status, body) = d
		.workspace_on(&bob, &bob_context, Some("able-oak-00000000"))
		.await;
	assert_eq!(status, 404, "{body}");

	d.withdraw_node_use(&tenant).await;

	let (status, body) = d.workspace_on(&bob, &bob_context, Some(&second)).await;
	assert_eq!(status, 403, "a member without node.use: {body}");
	let (status, body) = d.workspace_on(&bob, &bob_context, None).await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = d.workspace_on(&alice, &context, Some(&second)).await;
	assert_eq!(status, 200, "an administrator holds every action: {body}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn catalog_approvals_belong_to_one_node(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let first = d.nodes_of(&alice, &context, &tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let (_, created) = d.create_node(&alice, &context, &tenant, "Build").await;
	let second = created["name"].as_str().unwrap().to_owned();
	let (status, issued) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/authorization/{tenant}/credentials"),
			json!({"subject":"owner"}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let token = issued["token"].as_str().unwrap().to_owned();
	let send = async |method: &str, path: String, body: Value| {
		let (status, body, _) = d.send(method, &path, None, None, Some(&token), body).await;
		(status, body)
	};
	let (status, entry) = send(
		"POST",
		"/api/registry".into(),
		json!({"id":"probe","version":"1.0.0","kind":"skill","name":{"en":"Probe"},
			"description":{"en":"Per Node approval"},"config":{"instructions":"Use"}}),
	)
	.await;
	assert_eq!(status, 200, "{entry}");
	let approve = |node: &str, enabled: bool, revision: i64| {
		json!({"node":node,"approvals":[{"entry":{"id":"probe","version":"1.0.0"},
			"enabled":enabled,"expected_revision":revision}]})
	};
	let catalog = async |node: &str| {
		let (status, body) = send(
			"GET",
			format!("/api/authorization/{tenant}/catalog?node={node}"),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{body}");
		body.as_array().unwrap().clone()
	};

	let (status, bindings) = send(
		"POST",
		format!("/api/authorization/{tenant}/catalog/approvals"),
		approve(&first, true, 0),
	)
	.await;
	assert_eq!(status, 200, "{bindings}");
	assert_eq!(bindings[0]["node"], first);
	assert_eq!(catalog(&first).await.len(), 1);
	assert!(
		catalog(&second).await.is_empty(),
		"a new Node starts without approvals"
	);

	// The same exact version is a separate decision on the other Node, with its own revision.
	let (status, bindings) = send(
		"POST",
		format!("/api/authorization/{tenant}/catalog/approvals"),
		approve(&second, true, 0),
	)
	.await;
	assert_eq!(status, 200, "{bindings}");
	let (status, bindings) = send(
		"POST",
		format!("/api/authorization/{tenant}/catalog/approvals"),
		approve(&first, false, 1),
	)
	.await;
	assert_eq!(status, 200, "{bindings}");
	assert_eq!(catalog(&first).await[0]["enabled"], false);
	assert_eq!(catalog(&second).await[0]["enabled"], true);

	let (status, body) = send(
		"POST",
		format!("/api/authorization/{tenant}/catalog/approvals"),
		approve("able-oak-00000000", true, 0),
	)
	.await;
	assert_eq!(status, 404, "a Node of no Tenant: {body}");
	d.finish().await;
}

/// The HTTP lifecycle owns default safety, capacity release, and immutable
/// deletion history. Missing lifecycle routes or counting retained rows breaks
/// this contract; a pure repository test would miss authorization and routing.
#[rstest::rstest]
#[tokio::test]
async fn node_lifecycle_releases_capacity_and_keeps_each_deletion_history(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(
		runtime,
		TenancyRules {
			nodes_per_tenant: 2,
			..open_rules()
		},
	)
	.await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Lifecycle").await;
	let first = d.nodes_of(&alice, &context, &tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let path = format!("/api/tenants/{tenant}/nodes");
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{first}/deletion"),
			Some(&context),
			json!({"confirmation":first}),
		)
		.await;
	assert_eq!(status, 409, "default/last Node cannot be deleted: {body}");
	let (status, second) = d.create_node(&alice, &context, &tenant, "Second").await;
	assert_eq!(status, 201, "{second}");
	let second = second["name"].as_str().unwrap();
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{second}/deletion"),
			Some(&context),
			json!({"confirmation":"wrong"}),
		)
		.await;
	assert_eq!(status, 400, "{body}");
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{second}/default"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["default"], true);
	let (status, target_workspace) = d.workspace_on(&alice, &context, Some(&first)).await;
	assert_eq!(status, 200, "{target_workspace}");
	let (status, sibling_workspace) = d.workspace_on(&alice, &context, Some(second)).await;
	assert_eq!(status, 200, "{sibling_workspace}");
	let (status, deleted) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{first}/deletion"),
			Some(&context),
			json!({"confirmation":first}),
		)
		.await;
	assert_eq!(status, 200, "{deleted}");
	assert_eq!(deleted["state"], "deleted");
	// Cached browser context and an existing Workspace ID cannot retain deleted Node authority.
	for (workspace, expected) in [(&target_workspace, 403), (&sibling_workspace, 200)] {
		let (status, body) = d
			.as_person(
				&alice,
				"GET",
				&format!("/api/workspaces/{}", workspace["id"].as_str().unwrap()),
				Some(&context),
				Value::Null,
			)
			.await;
		assert_eq!(status, expected, "{body}");
	}
	let (status, body) = d.workspace_on(&alice, &context, Some(&first)).await;
	assert_eq!(status, 404, "deleted Node cannot create fresh work: {body}");

	let deadline = deleted["deletion"]["purge_after"].clone();
	assert!(deadline.is_string(), "{deleted}");
	assert_eq!(d.nodes_of(&alice, &context, &tenant).await.len(), 1);
	let (status, third) = d
		.create_node(&alice, &context, &tenant, "Reuses slot")
		.await;
	assert_eq!(
		status, 201,
		"deleted Node releases its slot immediately: {third}"
	);
	let third = third["name"].as_str().unwrap();
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{first}/restore"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(
		status, 409,
		"restore must respect the current limit: {body}"
	);
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{third}/deletion"),
			Some(&context),
			json!({"confirmation":third}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, restored) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{first}/restore"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{restored}");
	assert_eq!(restored["state"], "live");
	let (status, deleted_again) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{first}/deletion"),
			Some(&context),
			json!({"confirmation":first}),
		)
		.await;
	assert_eq!(status, 200, "{deleted_again}");
	assert_ne!(deleted_again["deletion"]["id"], deleted["deletion"]["id"]);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_deletions WHERE tenant = '{tenant}' AND node = '{first}'"
		))
		.await,
		2
	);
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{first}/purge"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(
		status, 403,
		"only Deployment Administrators request immediate purge: {body}"
	);
	let (status, requested) = d
		.as_deployment_administrator("POST", &format!("{path}/{first}/purge"), Value::Null)
		.await;
	assert_eq!(status, 200, "{requested}");
	assert_eq!(requested["state"], "deleting");
	assert_eq!(
		requested["deletion"]["purge_after"],
		deleted_again["deletion"]["purge_after"]
	);
	let (status, body) = d
		.as_deployment_administrator("POST", &format!("{path}/{first}/restore"), Value::Null)
		.await;
	assert_eq!(status, 409, "requested purge cannot be restored: {body}");
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM nodes WHERE tenant = '{tenant}' AND name = '{first}'"
		))
		.await,
		1,
		"purge endpoint only requests worker execution"
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn node_lifecycle_management_and_restore_use_current_collection_authority(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (tenant, context) = d.node_tenant(&alice, "Policy").await;
	let bob_context = d.admit(&tenant, &bob, "bob", json!(["members"])).await;
	let (status, created) = d
		.create_node(&alice, &context, &tenant, "Recoverable")
		.await;
	assert_eq!(status, 201, "{created}");
	let node = created["name"].as_str().unwrap();
	let path = format!("/api/tenants/{tenant}/nodes");
	let (status, ordinary) = d
		.as_person(
			&alice,
			"GET",
			&format!("{path}?limit=1&offset=1"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{ordinary}");
	assert_eq!(
		ordinary.as_array().unwrap().len(),
		2,
		"management pagination must not hide live Nodes from ordinary selectors"
	);
	let manage = format!("{path}?include_deleted=true&limit=1&offset=0");
	let (status, body) = d
		.as_person(&bob, "GET", &manage, Some(&bob_context), Value::Null)
		.await;
	assert_eq!(
		status, 403,
		"member read is not deletion management authority: {body}"
	);
	let (status, body) = d
		.as_person(
			&bob,
			"POST",
			&format!("{path}/{node}/deletion"),
			Some(&bob_context),
			json!({"confirmation":node}),
		)
		.await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{node}/deletion"),
			Some(&context),
			json!({"confirmation":node}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = d
		.as_person(
			&alice,
			"GET",
			&format!("{path}?include_deleted=true"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body.as_array().unwrap().len(), 2);
	assert!(
		body.as_array()
			.unwrap()
			.iter()
			.any(|record| record["name"] == node && record["storage_retained_until_purge"] == true)
	);
	let (status, body) = d
		.as_person(&alice, "GET", &manage, Some(&context), Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body.as_array().unwrap().len(), 1);
	let (status, body) = d
		.as_person(
			&alice,
			"GET",
			&format!("{path}?include_deleted=true&limit=101"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 400, "{body}");

	let policy_path = format!("/api/authorization/{tenant}");
	let (status, snapshot) = d
		.as_deployment_administrator("GET", &policy_path, Value::Null)
		.await;
	assert_eq!(status, 200, "{snapshot}");
	let mut bundle = snapshot["bundle"].clone();
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"withdraw-node-deletion", "effect":"deny", "subjects":{"groups":["administrators"]},
		"actions":["node.delete"], "resources":{"kinds":["node"]}
	}));
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&policy_path,
			json!({"expected_revision":snapshot["revision"],"bundle":bundle}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{node}/restore"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 403, "restore uses intervening policy edits: {body}");
	let (status, body) = d
		.as_person(&alice, "GET", &manage, Some(&context), Value::Null)
		.await;
	assert_eq!(
		status, 403,
		"management rechecks collection authority: {body}"
	);
	let (status, body) = d
		.as_deployment_administrator("POST", &format!("{path}/{node}/restore"), Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND node = '{node}' AND action IN ('node.create','node.delete','node.restore')")).await, 3);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn node_lifecycle_browser_administrator_keeps_identity_and_requires_current_grant(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let carol = d.person("carol").await;
	sqlx::query("INSERT INTO dashboard_deployment_administrator_grants (identity_id) VALUES ($1)")
		.bind(carol.identity)
		.execute(d.federation.store.pool.driver())
		.await
		.unwrap();
	let (tenant, context) = d.node_tenant(&alice, "Browser management").await;
	let first = d.nodes_of(&alice, &context, &tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let (status, second) = d
		.create_node(&alice, &context, &tenant, "New default")
		.await;
	assert_eq!(status, 201, "{second}");
	let second = second["name"].as_str().unwrap();
	let path = format!("/api/tenants/{tenant}/nodes");
	let (status, body) = d
		.as_person(
			&carol,
			"POST",
			&format!("{path}/{second}/default"),
			Some("deployment_administrator"),
			Value::Null,
		)
		.await;
	assert_eq!(
		status, 200,
		"browser administrator manages the Node default: {body}"
	);
	for suffix in ["purge/extra", "%70urge", "%2Fpurge"] {
		let (status, body) = d
			.as_person(
				&carol,
				"POST",
				&format!("{path}/{first}/{suffix}"),
				Some("deployment_administrator"),
				Value::Null,
			)
			.await;
		assert!(
			matches!(status, 403 | 404 | 405),
			"noncanonical lifecycle path must be refused: {status} {body}"
		);
	}
	let (status, body) = d
		.as_person(
			&carol,
			"POST",
			&format!("{path}/{first}/deletion"),
			Some("deployment_administrator"),
			json!({"confirmation":first}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		body["deletion"]["deleted_by_kind"],
		"deployment_administrator"
	);
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("{path}/{first}/restore"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(
		status, 409,
		"only a Deployment Administrator restores its deletion: {body}"
	);
	let (status, body) = d
		.as_person(
			&carol,
			"POST",
			&format!("{path}/{first}/restore"),
			Some("deployment_administrator"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = d
		.as_person(
			&carol,
			"POST",
			&format!("{path}/{first}/deletion"),
			Some("deployment_administrator"),
			json!({"confirmation":first}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = d
		.as_person(
			&carol,
			"POST",
			&format!("{path}/{first}/purge"),
			Some("deployment_administrator"),
			Value::Null,
		)
		.await;
	assert_eq!(
		status, 200,
		"browser administrator may request immediate purge: {body}"
	);
	let history = format!(
		"SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND actor_identity_id = '{}' AND node IS NOT NULL AND action IN ('node.default','node.delete','node.restore','node.purge')",
		carol.identity
	);
	assert_eq!(d.count(&history).await, 5);
	sqlx::query("UPDATE dashboard_deployment_administrator_grants SET enabled=false,revision=revision+1 WHERE identity_id=$1")
		.bind(carol.identity).execute(d.federation.store.pool.driver()).await.unwrap();
	let (status, body) = d
		.as_person(
			&carol,
			"POST",
			&format!("{path}/{second}/default"),
			Some("deployment_administrator"),
			Value::Null,
		)
		.await;
	assert_eq!(
		status, 403,
		"a disabled grant cannot retain browser management authority: {body}"
	);
	assert_eq!(d.count(&history).await, 5);
	d.finish().await;
}
