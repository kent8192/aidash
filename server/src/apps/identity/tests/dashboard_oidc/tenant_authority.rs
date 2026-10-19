//! A Tenant decides its own policy, Subject Credentials and Peer Mappings
//! through its members' Mappings, without a Deployment Administrator (#363).
use super::*;

/// A Tenant created by alice, who administers it, with bob as a member.
struct Team {
	d: Deployment,
	tenant: String,
	alice: Person,
	alice_context: String,
	bob: Person,
	bob_context: String,
}

impl Team {
	async fn new(runtime: common::RuntimeFuture) -> Self {
		let d = Deployment::new(runtime, open_rules()).await;
		let alice = d.person("alice").await;
		let bob = d.person("bob").await;
		let (status, created) = d
			.as_person(
				&alice,
				"POST",
				"/api/tenants",
				None,
				json!({"label":"Team"}),
			)
			.await;
		assert_eq!(status, 201, "{created}");
		let tenant = created["name"].as_str().unwrap().to_owned();
		let alice_context = d.context(&alice, &tenant).await;
		let bob_context = d.admit(&tenant, &bob, "bob", json!(["members"])).await;
		Self {
			d,
			tenant,
			alice,
			alice_context,
			bob,
			bob_context,
		}
	}

	fn path(&self, rest: &str) -> String {
		format!("/api/authorization/{}{rest}", self.tenant)
	}

	async fn alice(&self, method: &str, rest: &str, body: Value) -> (u16, Value) {
		self.d
			.as_person(
				&self.alice,
				method,
				&self.path(rest),
				Some(&self.alice_context),
				body,
			)
			.await
	}

	async fn bob(&self, method: &str, rest: &str, body: Value) -> (u16, Value) {
		self.d
			.as_person(
				&self.bob,
				method,
				&self.path(rest),
				Some(&self.bob_context),
				body,
			)
			.await
	}

	async fn bearer(&self, token: &str, method: &str, path: &str, body: Value) -> u16 {
		self.d
			.send(method, path, None, None, Some(token), body)
			.await
			.0
	}

	/// Replaces the policy as alice after `change` edits the current bundle.
	async fn replace(&self, change: impl FnOnce(&mut Value)) -> (u16, Value) {
		let (status, policy) = self.alice("GET", "", Value::Null).await;
		assert_eq!(status, 200, "{policy}");
		let mut bundle = policy["bundle"].clone();
		change(&mut bundle);
		self.alice(
			"POST",
			"",
			json!({"expected_revision":policy["revision"],"bundle":bundle}),
		)
		.await
	}

	async fn denials(&self, subject: &str, action: &str) -> i64 {
		self.d
			.count(&format!(
				"SELECT count(*) FROM authorization_decisions WHERE tenant = '{}' \
				 AND subject = '{subject}' AND action = '{action}' \
				 AND NOT (decision->>'allowed')::boolean",
				self.tenant
			))
			.await
	}

	fn mapping(context: &str) -> &str {
		context.rsplit(':').next().unwrap()
	}

	async fn finish(self) {
		self.d.finish().await;
	}
}

fn simulation(tenant: &str, subject: &str) -> Value {
	json!({
		"subject":subject,"action":"workspace.read",
		"resource":{"tenant":tenant,"kind":"workspace","id":"w","attributes":{}},
		"environment":{}
	})
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_administrator_reads_simulates_and_replaces_its_policy(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, policy) = t.alice("GET", "", Value::Null).await;
	assert_eq!(status, 200, "{policy}");
	assert_eq!(
		policy["bundle"]["subjects"]["bob"]["groups"],
		json!(["members"])
	);
	let (status, revisions) = t.alice("GET", "/revisions", Value::Null).await;
	assert_eq!(status, 200, "{revisions}");
	assert!(!revisions.as_array().unwrap().is_empty());

	let (status, decision) = t
		.alice("POST", "/simulate", simulation(&t.tenant, "bob"))
		.await;
	assert_eq!(status, 200, "{decision}");
	assert_eq!(decision["allowed"], true);
	// A simulation never enters the decision log, and a persisted evaluation,
	// which would, stays with Deployment Administrators.
	let (status, decisions) = t.alice("GET", "/decisions", Value::Null).await;
	assert_eq!(status, 200, "{decisions}");
	assert!(
		decisions
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["action"] != "workspace.read"),
		"{decisions}"
	);
	let (status, _) = t
		.alice("POST", "/evaluate", simulation(&t.tenant, "bob"))
		.await;
	assert_eq!(status, 403);

	let (status, replaced) = t
		.replace(|bundle| {
			bundle["groups"]["reviewers"] = json!({"roles":[],"assignable":true});
		})
		.await;
	assert_eq!(status, 200, "{replaced}");
	assert_eq!(
		replaced["revision"],
		policy["revision"].as_i64().unwrap() + 1
	);
	let stale = t
		.alice(
			"POST",
			"",
			json!({"expected_revision":policy["revision"],"bundle":replaced["bundle"]}),
		)
		.await;
	assert_eq!(stale.0, 409, "{}", stale.1);
	assert_eq!(
		t.d.count(&format!(
			"SELECT count(*) FROM authorization_revisions WHERE tenant = '{}' \
			 AND revision = {} AND actor = 'oidc:{}' AND actor_mapping_id = '{}'",
			t.tenant,
			replaced["revision"],
			t.alice.identity,
			Team::mapping(&t.alice_context)
		))
		.await,
		1
	);
	assert_eq!(
		t.d.count(&format!(
			"SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{}' \
			 AND action = 'policy.update' AND actor_kind = 'tenant_administrator' \
			 AND actor_identity_id = '{}'",
			t.tenant, t.alice.identity
		))
		.await,
		1
	);
	t.finish().await;
}

#[rstest::rstest]
#[case::leaves_no_administrator(json!([]))]
#[case::cannot_repair_the_policy(json!(["administrators"]))]
#[tokio::test]
async fn a_replacement_must_leave_someone_who_can_repair_the_policy(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[case] owner_groups: Value,
) {
	let t = Team::new(runtime).await;
	let (status, body) = t
		.replace(|bundle| {
			bundle["subjects"]["owner"]["groups"] = owner_groups.clone();
			if owner_groups != json!([]) {
				bundle["policies"].as_array_mut().unwrap().push(json!({
					"id":"no-policy-writes","effect":"deny","subjects":{"ids":["owner"]},
					"actions":["policy.update"],"resources":{"kinds":["policy"]}
				}));
			}
		})
		.await;
	assert_eq!(status, 409, "{body}");
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn members_without_the_actions_are_refused_with_an_audit(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	for (method, rest, action) in [
		("GET", "", "policy.read"),
		("GET", "/revisions", "policy.read"),
		("POST", "/simulate", "policy.read"),
		("GET", "/decisions", "decision.read"),
		("GET", "/credentials", "credential.read"),
		("GET", "/peer-mappings", "peer_mapping.read"),
	] {
		let before = t.denials("bob", action).await;
		let body = if method == "POST" {
			simulation(&t.tenant, "bob")
		} else {
			Value::Null
		};
		let (status, response) = t.bob(method, rest, body).await;
		assert_eq!(status, 403, "{method} {rest}: {response}");
		assert_eq!(t.denials("bob", action).await, before + 1, "{action}");
	}
	let (status, policy) = t.alice("GET", "", Value::Null).await;
	assert_eq!(status, 200);
	let (status, _) = t
		.bob(
			"POST",
			"",
			json!({"expected_revision":policy["revision"],"bundle":policy["bundle"]}),
		)
		.await;
	assert_eq!(status, 403);
	// Another Tenant's policy is refused whatever this one allows.
	let (status, other) =
		t.d.as_deployment_administrator("POST", "/api/tenants", json!({"label":"Other"}))
			.await;
	assert_eq!(status, 201, "{other}");
	let (status, _) =
		t.d.as_person(
			&t.alice,
			"GET",
			&format!("/api/authorization/{}", other["name"].as_str().unwrap()),
			Some(&t.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 403);
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_member_issues_credentials_for_itself_that_end_with_its_mapping(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, issued) = t
		.bob(
			"POST",
			"/credentials",
			json!({"subject":"bob","expires_in_seconds":3600}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	assert_eq!(
		issued["credential"]["mapping_id"],
		Team::mapping(&t.bob_context)
	);
	let token = issued["token"].as_str().unwrap().to_owned();
	assert_eq!(
		t.bearer(&token, "GET", "/api/state", Value::Null).await,
		200
	);
	// A credential never issues another, and a member issues none for others.
	assert_eq!(
		t.bearer(
			&token,
			"POST",
			&t.path("/credentials"),
			json!({"subject":"bob"})
		)
		.await,
		403
	);
	let (status, _) = t
		.bob("POST", "/credentials", json!({"subject":"owner"}))
		.await;
	assert_eq!(status, 403);
	let (status, mine) = t.bob("GET", "/credentials?mine=true", Value::Null).await;
	assert_eq!(status, 200, "{mine}");
	assert_eq!(mine.as_array().unwrap().len(), 1);
	let (status, all) = t.alice("GET", "/credentials", Value::Null).await;
	assert_eq!(status, 200, "{all}");
	assert!(
		all.as_array()
			.unwrap()
			.iter()
			.any(|row| row["id"] == issued["credential"]["id"])
	);
	for _ in 1..20 {
		let (status, body) = t
			.bob("POST", "/credentials", json!({"subject":"bob"}))
			.await;
		assert_eq!(status, 200, "{body}");
	}
	let (status, body) = t
		.bob("POST", "/credentials", json!({"subject":"bob"}))
		.await;
	assert_eq!(status, 409, "a Mapping holds at most 20: {body}");

	let (status, mappings) =
		t.d.as_person(
			&t.alice,
			"GET",
			&format!("/api/tenants/{}/mappings", t.tenant),
			Some(&t.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{mappings}");
	let bob = mappings
		.as_array()
		.unwrap()
		.iter()
		.find(|mapping| mapping["subject"] == "bob")
		.unwrap()
		.clone();
	let (status, body) =
		t.d.as_person(
			&t.alice,
			"POST",
			&format!(
				"/api/tenants/{}/mappings/{}/disable",
				t.tenant,
				bob["id"].as_str().unwrap()
			),
			Some(&t.alice_context),
			json!({"expected_revision":bob["revision"]}),
		)
		.await;
	assert_eq!(status, 204, "{body}");
	assert_eq!(
		t.d.count(&format!(
			"SELECT count(*) FROM authorization_credentials WHERE mapping_id = '{}' \
			 AND revoked_at IS NULL",
			bob["id"].as_str().unwrap()
		))
		.await,
		0
	);
	assert_eq!(
		t.bearer(&token, "GET", "/api/state", Value::Null).await,
		401
	);
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_administrator_issues_credentials_only_for_subjects_no_person_was_mapped_to(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, body) = t
		.replace(|bundle| {
			bundle["subjects"]["ci"] = json!({"kind":"service","groups":["members"]});
			// A person's subject stays theirs even after its kind changes.
			bundle["subjects"]["bob"]["kind"] = json!("service");
		})
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, issued) = t
		.alice("POST", "/credentials", json!({"subject":"ci"}))
		.await;
	assert_eq!(status, 200, "{issued}");
	assert_eq!(issued["credential"]["mapping_id"], Value::Null);
	assert_eq!(
		issued["credential"]["issued_by"],
		format!("oidc:{}", t.alice.identity)
	);
	let (status, body) = t
		.alice("POST", "/credentials", json!({"subject":"bob"}))
		.await;
	assert_eq!(status, 400, "{body}");
	let (status, revoked) = t
		.alice(
			"POST",
			&format!(
				"/credentials/{}/revoke",
				issued["credential"]["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{revoked}");
	assert!(revoked["revoked_at"].is_string());
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_peer_mapping_binds_only_a_credential_its_author_could_issue(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	sqlx::query(
		"INSERT INTO peers (node_id, endpoint, credential_env, protocol_version, enabled) \
		 VALUES ('aidash://node-remote', 'https://remote.example', 'AIDASH_SECRET_TEST_PEER', '0.1', true)",
	)
	.execute(t.d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let (status, body) = t
		.replace(|bundle| {
			bundle["subjects"]["ci"] = json!({"kind":"service","groups":["members"]});
		})
		.await;
	assert_eq!(status, 200, "{body}");
	let (_, service) = t
		.alice("POST", "/credentials", json!({"subject":"ci"}))
		.await;
	let (_, own) = t
		.bob("POST", "/credentials", json!({"subject":"bob"}))
		.await;
	let mapping = |credential: &Value, revision: i64, enabled: bool| {
		json!({"source_node":"aidash://node-remote","source_tenant":"remote","source_subject":"agent",
			"credential_id":credential["credential"]["id"],"enabled":enabled,
			"expected_revision":revision})
	};

	let (status, body) = t
		.bob("POST", "/peer-mappings", mapping(&service, 0, true))
		.await;
	assert_eq!(status, 403, "{body}");
	// A member's own credential would let someone outside act as that person,
	// and nobody but that person issues one for it.
	let (status, body) = t
		.alice("POST", "/peer-mappings", mapping(&own, 0, true))
		.await;
	assert_eq!(status, 400, "{body}");
	let (status, created) = t
		.alice("POST", "/peer-mappings", mapping(&service, 0, true))
		.await;
	assert_eq!(status, 200, "{created}");
	assert_eq!(created["actor"], format!("oidc:{}", t.alice.identity));
	let (status, history) = t.alice("GET", "/peer-mapping-history", Value::Null).await;
	assert_eq!(status, 200, "{history}");
	assert_eq!(history.as_array().unwrap().len(), 1);
	let (status, _) = t
		.alice("POST", "/peer-mappings", mapping(&service, 0, true))
		.await;
	assert_eq!(status, 409, "a stale revision conflicts");
	let (status, disabled) = t
		.alice("POST", "/peer-mappings", mapping(&service, 1, false))
		.await;
	assert_eq!(status, 200, "{disabled}");
	let revision = disabled["revision"].as_i64().unwrap();
	// A disabled mapping takes no credential an enabled one could not.
	let (status, body) = t
		.alice("POST", "/peer-mappings", mapping(&own, revision, false))
		.await;
	assert_eq!(status, 403, "{body}");
	// No one binds a Mapping's own credential, Deployment Administrators included.
	let mapping_credential: Uuid =
		sqlx::query_scalar("SELECT credential_id FROM dashboard_mappings WHERE id = $1::uuid")
			.bind(Team::mapping(&t.alice_context))
			.fetch_one(t.d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	let (status, body) =
		t.d.as_deployment_administrator(
			"POST",
			&t.path("/peer-mappings"),
			json!({"source_node":"aidash://node-remote","source_tenant":"remote","source_subject":"other",
				"credential_id":mapping_credential,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 403, "{body}");
	let mut disabled = mapping(&service, revision, false);
	disabled["credential_id"] = json!(mapping_credential);
	let (status, body) =
		t.d.as_deployment_administrator("POST", &t.path("/peer-mappings"), disabled)
			.await;
	assert_eq!(status, 403, "{body}");
	t.finish().await;
}
