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
async fn active_self_issued_credentials_are_bounded_independently_of_history(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	use aidash_server::apps::identity::models::AuthorizationCredential;
	use reinhardt::db::orm::Model;
	let t = Team::new(runtime).await;
	let active_id = Uuid::new_v4();
	let mapping = Team::mapping(&t.bob_context).parse::<Uuid>().unwrap();
	t.d.federation
		.store
		.pool
		.atomic(&aidash_application::tenancy::TEST_FIXTURE, async |tx| {
			let now = chrono::Utc::now();
			for n in 0_u32..202 {
				let entropy = Uuid::new_v4();
				let row = AuthorizationCredential::build()
					.id(if n == 0 { active_id } else { Uuid::new_v4() })
					.tenant(t.tenant.clone())
					.subject("bob")
					.token_hash(entropy.as_bytes().repeat(2))
					.created_at(now - chrono::Duration::hours(if n == 0 { 2 } else { 1 }))
					.expires_at(if n > 0 && n.is_multiple_of(2) {
						now - chrono::Duration::minutes(30)
					} else {
						now + chrono::Duration::hours(1)
					})
					.revoked_at(if n.is_multiple_of(2) {
						None
					} else {
						Some(now - chrono::Duration::minutes(10))
					})
					.issued_by("history-fixture")
					.mapping_id(Some(mapping))
					.finish();
				AuthorizationCredential::objects()
					.create_with_conn(tx, &row)
					.await?;
			}
			Ok(())
		})
		.await
		.unwrap();
	let (status, history) = t.bob("GET", "/credentials?mine=true", Value::Null).await;
	assert_eq!(status, 200, "{history}");
	assert_eq!(history.as_array().unwrap().len(), 200);
	assert!(
		history
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["id"] != json!(active_id))
	);
	let (status, active) = t
		.bob("GET", "/credentials?mine=true&active=true", Value::Null)
		.await;
	assert_eq!(status, 200, "{active}");
	assert_eq!(active.as_array().unwrap().len(), 1);
	assert_eq!(active[0]["id"], json!(active_id));
	let (status, page) = t
		.bob("GET", "/credentials?mine=true&offset=200", Value::Null)
		.await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page.as_array().unwrap().len(), 2);
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
	// A Node of another Tenant, registered at this Tenant's default Node.
	let (_, _, _, remote) = t.d.nodes_tenant("Remote").await;
	let receiving =
		t.d.default_node(&t.alice, &t.alice_context, &t.tenant)
			.await;
	t.d.accepts(&t.alice, &t.alice_context, &t.tenant, &remote)
		.await;
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
		json!({"node":receiving,"source_node":remote,"source_subject":"agent",
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
			json!({"node":receiving,"source_node":remote,"source_subject":"other",
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

#[rstest::rstest]
#[tokio::test]
async fn overview_lists_item_scoped_credential_targets_without_policy_read(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, response) = t.replace(|bundle| {
  bundle["subjects"]["ci"] = json!({"kind":"service"});
  bundle["subjects"]["other-ci"] = json!({"kind":"service"});
  bundle["policies"].as_array_mut().unwrap().extend([
   json!({"id":"issue-ci","effect":"allow","subjects":{"ids":["bob"]},"actions":["credential.issue"],"resources":{"kinds":["*"],"ids":["ci"]}}),
   json!({"id":"no-policy-read","effect":"deny","subjects":{"ids":["bob"]},"actions":["policy.read"],"resources":{"kinds":["*"]}}),
  ]);
 }).await;
	assert_eq!(status, 200, "{response}");
	let (status, overview) =
		t.d.as_person(
			&t.bob,
			"GET",
			&format!("/api/tenants/{}/administration", t.tenant),
			Some(&t.bob_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{overview}");
	assert_eq!(overview["issuable_subjects"], json!(["ci"]));
	assert!(
		!overview["actions"]
			.as_array()
			.unwrap()
			.contains(&json!("policy.read"))
	);
	assert!(
		!overview["actions"]
			.as_array()
			.unwrap()
			.contains(&json!("credential.issue")),
		"an item grant must not imply a collection grant"
	);
	assert_eq!(
		t.bob("POST", "/credentials", json!({"subject":"ci"}))
			.await
			.0,
		200
	);
	assert_eq!(
		t.bob("POST", "/credentials", json!({"subject":"other-ci"}))
			.await
			.0,
		403
	);
	t.finish().await;
}

/// A Deployment Administrator, by the shared bearer or by a browser context, is
/// refused on Tenant content and settings, and nothing changes (#484, #301).
#[rstest::rstest]
#[tokio::test]
async fn a_deployment_administrator_holds_no_tenant_content_or_settings_authority(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let carol = t.d.person("carol").await;
	sqlx::query("INSERT INTO dashboard_deployment_administrator_grants (identity_id) VALUES ($1)")
		.bind(carol.identity)
		.execute(t.d.federation.store.pool.driver())
		.await
		.unwrap();
	let scope = aidash_application::tenancy::TenancyScope::tenant(&t.tenant).unwrap();
	let store = &t.d.federation.store;
	let workspace = store
		.create_workspace(&scope, "alice", "Confidential plan", "Tenant content")
		.await
		.unwrap();
	let task = store
		.create_task(
			&scope,
			workspace.id,
			&aidash_domain::NewTask {
				title: "Confidential task".into(),
				description: "Tenant content".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"alice",
			None,
		)
		.await
		.unwrap();
	let tenant = &t.tenant;
	let workspace_path = format!("/api/workspaces/{}", workspace.id);
	let routes: Vec<(&str, String, Value)> = vec![
		("GET", workspace_path.clone(), Value::Null),
		("GET", "/api/tasks".into(), Value::Null),
		(
			"POST",
			format!("/api/tasks/{}/abandon", task.id),
			json!({"revision":task.revision,"reason":"administrator attempt"}),
		),
		(
			"GET",
			format!("{workspace_path}/message-history"),
			Value::Null,
		),
		(
			"GET",
			format!("{workspace_path}/attachments/{}", Uuid::new_v4()),
			Value::Null,
		),
		(
			"GET",
			format!("/api/authorization/{tenant}/credentials"),
			Value::Null,
		),
		(
			"POST",
			format!("/api/authorization/{tenant}/credentials"),
			json!({"subject":"bob"}),
		),
		(
			"POST",
			format!(
				"/api/authorization/{tenant}/credentials/{}/revoke",
				Uuid::new_v4()
			),
			Value::Null,
		),
		(
			"POST",
			format!("/api/authorization/{tenant}/simulate"),
			simulation(tenant, "bob"),
		),
		(
			"GET",
			format!("/api/authorization/{tenant}/revisions"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/authorization/{tenant}/decisions"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/authorization/{tenant}/peer-mappings"),
			Value::Null,
		),
		(
			"POST",
			format!("/api/authorization/{tenant}/peer-mappings"),
			json!({}),
		),
		("GET", format!("/api/tenants/{tenant}/nodes"), Value::Null),
		(
			"GET",
			format!(
				"/api/tenants/{tenant}/nodes/{}/registrations",
				aidash_domain::nodes::NodeName::derived("x")
			),
			Value::Null,
		),
		(
			"GET",
			format!("/api/tenants/{tenant}/provider-credentials"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/tenants/{tenant}/provider-credential-bindings"),
			Value::Null,
		),
		(
			"PUT",
			format!("/api/tenants/{tenant}/provider-credential-bindings/openrouter"),
			json!({"provider_credential_id":null,"expected_revision":0}),
		),
		(
			"POST",
			format!("/api/tenants/{tenant}/provider-authorizations/openrouter"),
			json!({}),
		),
	];
	let credentials_before =
		t.d.count(&format!(
			"SELECT count(*) FROM authorization_credentials WHERE tenant = '{tenant}'"
		))
		.await;
	for (method, path, body) in &routes {
		let (status, refused) =
			t.d.as_deployment_administrator(method, path, body.clone())
				.await;
		assert_eq!(status, 403, "bearer {method} {path}: {refused}");
		for secret in ["Confidential", "token"] {
			assert!(!refused.to_string().contains(secret), "{path}: {refused}");
		}
		let (status, refused) =
			t.d.as_person(
				&carol,
				method,
				path,
				Some("deployment_administrator"),
				body.clone(),
			)
			.await;
		assert_eq!(status, 403, "browser {method} {path}: {refused}");
		for secret in ["Confidential", "token"] {
			assert!(!refused.to_string().contains(secret), "{path}: {refused}");
		}
	}
	// Nothing changed: no credential was issued and the Task was not abandoned.
	assert_eq!(
		t.d.count(&format!(
			"SELECT count(*) FROM authorization_credentials WHERE tenant = '{tenant}'"
		))
		.await,
		credentials_before
	);
	assert_eq!(
		t.d.count(&format!(
			"SELECT count(*) FROM tasks WHERE id = '{}' AND status = 'ABANDONED'",
			task.id
		))
		.await,
		0,
		"the refused abandon left the Task alone"
	);
	// The state projection carries no Tenant collections to an administrator.
	for state in [
		t.d.as_deployment_administrator("GET", "/api/state", Value::Null)
			.await,
		t.d.as_person(
			&carol,
			"GET",
			"/api/state",
			Some("deployment_administrator"),
			Value::Null,
		)
		.await,
	] {
		assert_eq!(state.0, 200, "{}", state.1);
		for collection in [
			"workspaces",
			"tasks",
			"runs",
			"events",
			"artifacts",
			"conversations",
		] {
			assert_eq!(state.1[collection], json!([]), "{collection}: {}", state.1);
		}
		assert!(!state.1.to_string().contains("Confidential"));
	}
	// An authorized Tenant Subject, by Mapping or by Subject Credential, still succeeds.
	let (status, mine) = t.alice("GET", "", Value::Null).await;
	assert_eq!(status, 200, "{mine}");
	let (status, body, _) =
		t.d.send(
			"GET",
			&workspace_path,
			Some(&t.alice),
			Some(&t.alice_context),
			None,
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert!(body.to_string().contains("Confidential plan"), "{body}");
	let token = common::subject_credential(store, tenant, "owner").await;
	let (status, body, _) =
		t.d.send("GET", "/api/tasks", None, None, Some(&token), Value::Null)
			.await;
	assert_eq!(status, 200, "{body}");
	assert!(body.to_string().contains("Confidential task"), "{body}");
	// A Subject of a second Tenant is refused on this Tenant's content.
	let dave = t.d.person("dave").await;
	let (other, other_context) = t.d.tenant_of(&dave, "Elsewhere").await;
	assert_ne!(&other, tenant);
	let (status, body, _) =
		t.d.send(
			"GET",
			&workspace_path,
			Some(&dave),
			Some(&other_context),
			None,
			Value::Null,
		)
		.await;
	assert!(matches!(status, 403 | 404), "{status} {body}");
	assert!(!body.to_string().contains("Confidential"));
	let (status, body, _) =
		t.d.send(
			"GET",
			"/api/state",
			Some(&dave),
			Some(&other_context),
			None,
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert!(!body.to_string().contains("Confidential"), "{body}");
	t.finish().await;
}
