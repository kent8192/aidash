//! Agents create and delete Nodes as their root person, the org chart shows
//! what each flow made, and one action stops a flow (#1012, ADR 0143).
use super::*;
use aidash_server::apps::identity::services::nodes::agent_tool as node_tool;
use aidash_server::apps::workspaces::services::duties::agent_tool as duty_tool;

/// The fixture's policy with `deny` added: actions refused to `subjects`.
fn denying(c: &CoreFixture, id: &str, subjects: &[&str], actions: &[&str]) -> Value {
	let mut policy = c.policy.clone();
	policy["subjects"]["bob"] = json!({"kind":"user"});
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id": id,
		"effect": "deny",
		"subjects": {"ids": subjects},
		"actions": actions,
		"resources": {"kinds": ["*"]},
	}));
	policy
}

async fn creator(
	c: &CoreFixture,
	node: &str,
) -> (String, Option<String>, Option<Uuid>, Option<Uuid>) {
	sqlx::query_as(
		"SELECT created_by_kind, created_by_subject, created_by_task_id, created_by_run_id FROM nodes WHERE name = $1",
	)
	.bind(node)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

async fn live_nodes(c: &CoreFixture) -> i64 {
	sqlx::query_scalar(
		"SELECT count(*) FROM nodes n WHERE n.tenant = 'acme' AND NOT EXISTS (SELECT 1 FROM node_deletions d WHERE d.tenant = n.tenant AND d.node = n.name AND d.restored_at IS NULL)",
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn an_agent_creates_a_node_only_when_its_root_person_may(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: alice, the root person of the Run, holds every action.
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let before = live_nodes(&c).await;
	// Act
	let created = node_tool(
		&c.f,
		&run,
		"call-1",
		"node_create",
		json!({"label": "Research"}),
	)
	.await
	.unwrap();
	let retried = node_tool(
		&c.f,
		&run,
		"call-1",
		"node_create",
		json!({"label": "Research"}),
	)
	.await
	.unwrap();
	// Assert: one Node, recorded as the chain's, and a retry finds it.
	let name = created["node"].as_str().unwrap().to_owned();
	assert_eq!(created["label"], "Research");
	assert_eq!(retried["node"], created["node"]);
	assert_eq!(live_nodes(&c).await, before + 1);
	assert_eq!(
		creator(&c, &name).await,
		(
			"agent".to_owned(),
			Some("alice".to_owned()),
			Some(run.task_id),
			Some(run.id)
		)
	);
	// The Node received the Tenant's Catalog Approvals as a person's would.
	let approvals: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM authorization_catalog WHERE tenant = 'acme' AND node = $1",
	)
	.bind(&name)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let source: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM authorization_catalog WHERE tenant = 'acme' AND node = $1 AND scope = 'deployment'",
	)
	.bind(aidash_domain::nodes::NodeName::derived("acme").as_str())
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(approvals, source);
	// Once the policy refuses alice `node.create`, her Agent is refused too,
	// and the refusal is in the decision log.
	common::replace_policy(
		&c.f.store,
		"acme",
		denying(&c, "no-nodes", &["alice"], &["node.create"]),
	)
	.await;
	let refused = node_tool(
		&c.f,
		&run,
		"call-2",
		"node_create",
		json!({"label": "Sales"}),
	)
	.await;
	assert!(
		matches!(&refused, Err(aidash_server::Error::Invalid(message)) if message.contains("node.create")),
		"{refused:?}"
	);
	assert_eq!(live_nodes(&c).await, before + 1);
	let denials: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM authorization_decisions WHERE tenant = 'acme' AND action = 'node.create' AND NOT (decision->>'allowed')::boolean",
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(denials >= 1);
	c.close().await;
}

/// An Agent deletes a Node its own flow created, and no other: not one
/// another flow of the same person created, even though that person may
/// delete Nodes (#1012 decision 20).
#[rstest::rstest]
#[tokio::test]
async fn an_agent_deletes_its_own_flows_node_but_not_another_flows(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: two Nodes alice's flow created; one is then recorded as made by
	// another Root Task of hers in the same Thread. alice holds `node.delete`.
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let own = node_tool(&c.f, &run, "call-1", "node_create", json!({"label": "Own"}))
		.await
		.unwrap()["node"]
		.as_str()
		.unwrap()
		.to_owned();
	let others = node_tool(
		&c.f,
		&run,
		"call-2",
		"node_create",
		json!({"label": "Another flow's"}),
	)
	.await
	.unwrap()["node"]
		.as_str()
		.unwrap()
		.to_owned();
	let thread: Uuid = sqlx::query_scalar("SELECT thread_id FROM tasks WHERE id = $1")
		.bind(c.task)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (status, second) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/threads/{thread}/tasks"),
		json!({"title": "Another request", "description": "Unrelated"}),
	)
	.await;
	assert_eq!(status, 200, "{second}");
	let second: Uuid = serde_json::from_value(second["id"].clone()).unwrap();
	sqlx::query("UPDATE nodes SET created_by_task_id = $1 WHERE name = $2")
		.bind(second)
		.bind(&others)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	// Act
	let refused = node_tool(&c.f, &run, "call-3", "node_delete", json!({"node": others})).await;
	let deleted = node_tool(&c.f, &run, "call-4", "node_delete", json!({"node": own}))
		.await
		.unwrap();
	let default = node_tool(
		&c.f,
		&run,
		"call-5",
		"node_delete",
		json!({"node": aidash_domain::nodes::NodeName::derived("acme").as_str()}),
	)
	.await;
	// Assert: the other flow's Node stays; her flow's own is deleted, not purged.
	assert!(
		matches!(&refused, Err(aidash_server::Error::Invalid(message)) if message.contains("own flow")),
		"{refused:?}"
	);
	assert_eq!(deleted["deleted"], true);
	assert!(deleted["restorable_until"].is_string());
	let deletions: Vec<(String, String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
		"SELECT node, deleted_by_kind, purged_at FROM node_deletions WHERE tenant = 'acme' AND restored_at IS NULL",
	)
	.fetch_all(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(deletions, vec![(own.clone(), "agent".to_owned(), None)]);
	// The Node the Run works on, which a person created, is never its to delete.
	assert!(
		matches!(default, Err(aidash_server::Error::Invalid(_))),
		"{default:?}"
	);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn stopping_a_flow_cancels_its_runs_ends_its_subnodes_and_pauses_its_duties(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: the Root Task's Run sets up a Duty, creates a Subnode and a Node.
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let duty = duty_tool(
		&c.f.store,
		&run,
		"call-1",
		"duty_create",
		json!({"title": "Flow digest", "mission": "Collect the open questions."}),
	)
	.await
	.unwrap();
	let duty: Uuid = serde_json::from_value(duty["duty_id"].clone()).unwrap();
	let creating: Option<Uuid> =
		sqlx::query_scalar("SELECT created_by_task_id FROM duties WHERE id = $1")
			.bind(duty)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(creating, Some(c.task));
	let subnode = late_subnode::seed_subnode(&c.f, run.id, &[]).await;
	let node = node_tool(
		&c.f,
		&run,
		"call-2",
		"node_create",
		json!({"label": "Made by the flow"}),
	)
	.await
	.unwrap()["node"]
		.as_str()
		.unwrap()
		.to_owned();
	let path = format!("/api/tenants/acme/flows/{}/stop", c.task);
	// A Subject Credential, which an Agent's chain holds, never stops a flow.
	let (status, refused) = request(&c.app, &c.token, "POST", &path, Value::Null).await;
	assert_eq!(status, 403, "{refused}");
	// Act
	let browser = common::BrowserMapping::new(&c.f, "acme", "alice").await;
	let (status, stopped) = browser.send("POST", &path, Value::Null).await;
	// Assert
	assert_eq!(status, 200, "{stopped}");
	assert_eq!(stopped["root_task"], json!(c.task));
	assert_eq!(
		stopped["runs"],
		json!([{"id": run.id, "outcome": "stopped"}])
	);
	assert_eq!(
		stopped["duties"],
		json!([{"id": duty, "outcome": "stopped"}])
	);
	assert_eq!(stopped["nodes"][0]["name"], json!(node));
	let control: String = sqlx::query_scalar("SELECT control FROM task_executions WHERE id = $1")
		.bind(run.id)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(control, "CANCELLED");
	let state: String = sqlx::query_scalar("SELECT state FROM subnodes WHERE id = $1")
		.bind(subnode)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(state, "dissolved");
	let halt: Option<String> = sqlx::query_scalar("SELECT halt FROM duties WHERE id = $1")
		.bind(duty)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(halt.as_deref(), Some("paused"));
	// The Node it created stays.
	let deleted: i64 = sqlx::query_scalar("SELECT count(*) FROM node_deletions WHERE node = $1")
		.bind(&node)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(deleted, 0);
	// Stopping again finds everything already stopped.
	let (status, again) = browser.send("POST", &path, Value::Null).await;
	assert_eq!(status, 200, "{again}");
	assert_eq!(again["runs"][0]["outcome"], "already_stopped");
	assert_eq!(again["duties"][0]["outcome"], "already_stopped");
	c.close().await;
}

/// The state and end reason of a Subnode.
async fn subnode_end(c: &CoreFixture, subnode: Uuid) -> (String, Option<String>) {
	sqlx::query_as("SELECT state, ended_reason FROM subnodes WHERE id = $1")
		.bind(subnode)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// Disabling or removing the creating Agent deletes its Subnodes (#1012):
/// withdrawing the creating version's Catalog Approval on the Subnode's Node,
/// or disabling its Agent subject in the Tenant's policy, ends the Subnode
/// with `creator_agent_removed`. A policy save that leaves the Agent usable
/// ends nothing.
#[rstest::rstest]
#[case::approval_withdrawn(true)]
#[case::subject_disabled(false)]
#[tokio::test]
async fn disabling_the_creating_agent_deletes_its_subnodes(
	#[future] capability_fixture: CoreFixture,
	#[case] withdraw: bool,
) {
	// Arrange: the fixture's research@1.1.0 Run created a Subnode on the
	// Tenant's default Node.
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let subnode = late_subnode::seed_subnode(&c.f, run.id, &[]).await;
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let alice = common::BrowserMapping::new(&c.f, "acme", "alice").await;
	// A policy save that changes nothing about the Agent keeps the Subnode.
	let (status, current) = alice
		.send("GET", "/api/authorization/acme", Value::Null)
		.await;
	assert_eq!(status, 200, "{current}");
	let (status, saved) = alice
		.send(
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision": current["revision"], "bundle": current["bundle"]}),
		)
		.await;
	assert!(status == 200 || status == 202, "{saved}");
	assert_eq!(subnode_end(&c, subnode).await, ("active".into(), None));
	// Act
	if withdraw {
		let (status, bindings) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/authorization/acme/catalog?node={node}"),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{bindings}");
		let binding = bindings
			.as_array()
			.unwrap()
			.iter()
			.find(|binding| {
				binding["entry_id"] == "research" && binding["entry_version"] == "1.1.0"
			})
			.cloned()
			.unwrap_or_else(|| panic!("research@1.1.0 is approved: {bindings}"));
		let (status, withdrawn) = request(
			&c.app,
			&c.token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"node": node, "entry": {"id": "research", "version": "1.1.0"},
				"expected_revision": binding["revision"], "enabled": false, "scope": "node"}),
		)
		.await;
		assert_eq!(status, 200, "{withdrawn}");
		assert_eq!(withdrawn["enabled"], false, "{withdrawn}");
	} else {
		let (status, current) = alice
			.send("GET", "/api/authorization/acme", Value::Null)
			.await;
		assert_eq!(status, 200, "{current}");
		let subject = aidash_server::domain::qualified_agent(
			&aidash_domain::nodes::NodeAddress::derived(c.f.config.deployment.as_str(), "acme"),
			"research",
			"1.1.0",
		);
		let mut bundle = current["bundle"].clone();
		assert!(
			bundle["subjects"].get(&subject).is_some(),
			"the approved Agent is a subject: {bundle}"
		);
		bundle["subjects"][&subject]["enabled"] = json!(false);
		let (status, saved) = alice
			.send(
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision": current["revision"], "bundle": bundle}),
			)
			.await;
		assert!(status == 200 || status == 202, "{saved}");
	}
	// Assert: the Subnode ended as its creator's own end would, records kept.
	assert_eq!(
		subnode_end(&c, subnode).await,
		("dissolved".into(), Some("creator_agent_removed".into()))
	);
	let events: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM events WHERE kind = 'subnode.dissolved' \
		 AND data->>'subnode_id' = $1 AND data->>'reason' = 'creator_agent_removed'",
	)
	.bind(subnode.to_string())
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(events, 1);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_org_chart_shows_each_person_what_they_may_see(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: alice's chain creates a Node, sets up a Duty on its own Node
	// and forms a Subnode there, recorded by its Node's address as
	// `subnode_create` records it; bob is a member who may read only the
	// default Node, reads no Thread of hers and administers nothing.
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let node = node_tool(
		&c.f,
		&run,
		"call-1",
		"node_create",
		json!({"label": "Research"}),
	)
	.await
	.unwrap()["node"]
		.as_str()
		.unwrap()
		.to_owned();
	let duty = duty_tool(
		&c.f.store,
		&run,
		"call-2",
		"duty_create",
		json!({"title": "Chart digest", "mission": "Collect the open questions."}),
	)
	.await
	.unwrap();
	let subnode = late_subnode::seed_subnode(&c.f, run.id, &[]).await;
	let mut policy = denying(
		&c,
		"bob-member",
		&["bob"],
		&[
			"node.administer",
			"budget.manage",
			"duty.administer",
			"thread.read",
		],
	);
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id": "bob-not-research",
		"effect": "deny",
		"subjects": {"ids": ["bob"]},
		"actions": ["node.read"],
		"resources": {"kinds": ["node"], "ids": [node]},
	}));
	common::replace_policy(&c.f.store, "acme", policy.clone()).await;
	let path = "/api/tenants/acme/org-chart";
	// Act
	let alice = common::BrowserMapping::new(&c.f, "acme", "alice").await;
	let (status, full) = alice.send("GET", path, Value::Null).await;
	let bob = common::BrowserMapping::new(&c.f, "acme", "bob").await;
	let (bob_status, limited) = bob.send("GET", path, Value::Null).await;
	// Assert: alice sees the Node her chain created, its creator and flow.
	assert_eq!(status, 200, "{full}");
	let created = full["nodes"]
		.as_array()
		.unwrap()
		.iter()
		.find(|entry| entry["name"] == json!(node))
		.unwrap();
	assert_eq!(created["created_by"]["kind"], "agent");
	assert_eq!(created["created_by"]["root_subject"], "alice");
	assert_eq!(created["created_by"]["task_id"], json!(c.task));
	assert_eq!(created["flow"], json!(c.task));
	assert!(created["spend"].is_object(), "{created}");
	let home = full["nodes"]
		.as_array()
		.unwrap()
		.iter()
		.find(|entry| {
			entry["duties"]
				.as_array()
				.is_some_and(|duties| !duties.is_empty())
		})
		.unwrap();
	assert_eq!(home["duties"][0]["title"], "Chart digest");
	assert_eq!(home["duties"][0]["flow"], json!(c.task));
	assert_eq!(home["subnodes"][0]["id"], json!(subnode), "{home}");
	assert_eq!(home["subnodes"][0]["root_subject"], "alice");
	assert_eq!(home["subnodes"][0]["flow"], json!(c.task));
	assert_eq!(home["hidden_subnodes"], 0);
	// bob does not see that Node; on the Node he reads, alice's Duty is only
	// counted, and he sees no spend.
	assert_eq!(bob_status, 200, "{limited}");
	let names: Vec<&Value> = limited["nodes"]
		.as_array()
		.unwrap()
		.iter()
		.map(|entry| &entry["name"])
		.collect();
	assert!(!names.contains(&&json!(node)), "{limited}");
	let bob_home = limited["nodes"]
		.as_array()
		.unwrap()
		.iter()
		.find(|entry| entry["name"] == home["name"])
		.unwrap();
	assert_eq!(bob_home["duties"], json!([]));
	assert_eq!(bob_home["hidden_duties"], 1);
	assert_eq!(bob_home["subnodes"], json!([]));
	assert_eq!(bob_home["hidden_subnodes"], 1);
	assert!(bob_home.get("spend").is_none());
	assert_eq!(limited["people"], json!([]));
	// Give Bob access to the originating Thread while the Duty's own Thread
	// remains private. The chart now shows the same Duty as its detail page.
	let duty_id = duty["duty_id"].as_str().unwrap();
	let duty_thread: Uuid = serde_json::from_value(duty["thread_id"].clone()).unwrap();
	assert_ne!(run.thread_id, duty_thread);
	let policies = policy["policies"].as_array_mut().unwrap();
	let member = policies
		.iter_mut()
		.find(|rule| rule["id"] == "bob-member")
		.unwrap();
	member["actions"]
		.as_array_mut()
		.unwrap()
		.retain(|action| action != "thread.read");
	policies.push(json!({
		"id": "bob-not-duty-thread",
		"effect": "deny",
		"subjects": {"ids": ["bob"]},
		"actions": ["thread.read"],
		"resources": {"kinds": ["thread"], "ids": [duty_thread]},
	}));
	common::replace_policy(&c.f.store, "acme", policy).await;
	let (status, body) = bob
		.send(
			"GET",
			&format!("/api/threads/{}", run.thread_id),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = bob
		.send("GET", &format!("/api/threads/{duty_thread}"), Value::Null)
		.await;
	assert!(matches!(status, 403 | 404), "{status} {body}");
	let (status, body) = bob
		.send("GET", &format!("/api/duties/{duty_id}"), Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, with_origin) = bob.send("GET", path, Value::Null).await;
	assert_eq!(status, 200, "{with_origin}");
	let bob_home = with_origin["nodes"]
		.as_array()
		.unwrap()
		.iter()
		.find(|entry| entry["name"] == home["name"])
		.unwrap();
	assert_eq!(bob_home["duties"][0]["id"], duty["duty_id"], "{bob_home}");
	assert_eq!(bob_home["duties"][0]["flow"], json!(c.task));
	assert_eq!(bob_home["hidden_duties"], 0);
	// A Subject Credential reads no org chart.
	let (status, _) = request(&c.app, &c.token, "GET", path, Value::Null).await;
	assert_eq!(status, 403);
	c.close().await;
}

async fn notices(c: &CoreFixture, budget: Uuid) -> i64 {
	sqlx::query_scalar("SELECT count(*) FROM budget_notices WHERE budget_id = $1")
		.bind(budget)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// A Soft Limit mail the SMTP server refuses is not recorded as sent: the
/// next pass sends it again, and a sent one is not sent twice (#1012).
#[rstest::rstest]
#[tokio::test]
async fn a_soft_limit_mail_that_fails_is_sent_again_on_the_next_pass(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: a Tenant Budget past its Soft Limit, and an SMTP server that
	// refuses connections.
	let mut c = capability_fixture.await;
	let budget = Uuid::now_v7();
	sqlx::query("INSERT INTO budgets (id, tenant, scope_kind, scope_key, period, soft_limit_nanocredits, on_hard_limit, recipients, revision, created_at, created_by, updated_at) VALUES ($1, 'acme', 'tenant', '', 'monthly', 10, 'stop', '{ops@example.test}', 1, now(), 'alice', now())")
		.bind(budget)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	sqlx::query("INSERT INTO budget_spend (tenant, day, node, root_subject, duty_id, spent_nanocredits, unpriced_calls) VALUES ('acme', date_trunc('day', now(), 'UTC'), 'any', 'alice', '00000000-0000-0000-0000-000000000000', 10, 0)")
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let port = closed.local_addr().unwrap().port();
	drop(closed);
	c.f.config.smtp = Some(
		aidash_server::apps::operations::serializers::deployment::SmtpSettings {
			host: "127.0.0.1".into(),
			port,
			from: "budgets@example.test".into(),
			username: None,
			password: None,
			starttls: false,
		},
	);
	// Act
	let failed = aidash_server::apps::identity::services::budgets::notice_pass(&c.f)
		.await
		.unwrap();
	// Assert: nothing was sent, and the window's notice is still open.
	assert_eq!(failed, 0);
	assert_eq!(notices(&c, budget).await, 0);
	// Act: the next pass goes through (no SMTP settings: recorded only).
	c.f.config.smtp = None;
	let sent = aidash_server::apps::identity::services::budgets::notice_pass(&c.f)
		.await
		.unwrap();
	let again = aidash_server::apps::identity::services::budgets::notice_pass(&c.f)
		.await
		.unwrap();
	// Assert
	assert_eq!((sent, again), (1, 0));
	assert_eq!(notices(&c, budget).await, 1);
	c.close().await;
}
