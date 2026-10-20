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
	async fn node_tenant(&self, person: &Person, label: &str) -> (String, String) {
		let (status, created) = self
			.as_person(person, "POST", "/api/tenants", None, json!({"label":label}))
			.await;
		assert_eq!(status, 201, "{created}");
		let tenant = created["name"].as_str().unwrap().to_owned();
		let context = self.context(person, &tenant).await;
		(tenant, context)
	}

	async fn nodes_of(&self, person: &Person, context: &str, tenant: &str) -> Vec<Value> {
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

	async fn create_node(
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
