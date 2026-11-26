//! A Tenant decides its own policy, Subject Credentials and Peer Mappings
//! through its members' Mappings, without a Deployment Administrator (#363).
use super::*;

/// A Tenant created by alice, who administers it, with bob as a member.
struct Team {
	d: Deployment,
	tenant: String,
	node: String,
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
		let node = d.nodes_of(&alice, &alice_context, &tenant).await[0]["name"]
			.as_str()
			.unwrap()
			.to_owned();
		let bob_context = d.admit(&tenant, &bob, "bob", json!(["members"])).await;
		Self {
			d,
			tenant,
			node,
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
		"subject":subject,"action":"thread.read",
		"resource":{"tenant":tenant,"kind":"thread","id":"w","attributes":{}},
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
			.all(|row| row["action"] != "thread.read"),
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
		("GET", "/safety-stops", "decision.read"),
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
async fn safety_stops_are_audited_to_the_tenant_and_not_to_a_deployment_administrator(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, empty) = t.alice("GET", "/safety-stops", Value::Null).await;
	assert_eq!(status, 200, "{empty}");
	assert_eq!(empty, json!([]));
	let record = Uuid::new_v4();
	sqlx::query(
		"INSERT INTO run_safety_stops (id, tenant, run_id, action, expected_control_revision, actor_kind, identity_id, reason, operation_id, caller_key, fingerprint, outcome, created_at) \
		 VALUES ($1, $2, $3, 'pause', 0, 'shared_bearer', NULL, 'runaway spend', $4, 'shared_bearer', 'digest', 'accepted', clock_timestamp())",
	)
	.bind(record)
	.bind(&t.tenant)
	.bind(Uuid::new_v4())
	.bind(Uuid::new_v4())
	.execute(t.d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let (status, listed) = t.alice("GET", "/safety-stops", Value::Null).await;
	assert_eq!(status, 200, "{listed}");
	assert_eq!(listed[0]["id"], record.to_string());
	assert_eq!(listed[0]["reason"], "runaway spend");
	assert_eq!(listed[0]["actor_kind"], "shared_bearer");
	assert!(listed[0]["identity_id"].is_null());
	// Without `decision.read` the member is refused; so is a Deployment
	// Administrator, whose receipt grants no read of the Tenant's audit history.
	assert_eq!(t.bob("GET", "/safety-stops", Value::Null).await.0, 403);
	let (status, body) =
		t.d.as_deployment_administrator("GET", &t.path("/safety-stops"), Value::Null)
			.await;
	assert_eq!(status, 403, "{body}");
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
	// A credential that ended with its Mapping answers 403 (#876 decision 12).
	assert_eq!(
		t.bearer(&token, "GET", "/api/state", Value::Null).await,
		403
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
					.end_reason(None)
					.parent_id(None)
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
async fn tenant_owned_credential_may_issue_independent_credentials_under_policy(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, body) = t.replace(|bundle| {
		bundle["subjects"]["ci"] = json!({"kind":"service", "node":t.node, "groups":["members"]});
		bundle["subjects"]["other-ci"] = json!({"kind":"service", "node":t.node, "groups":["members"]});
		bundle["policies"].as_array_mut().unwrap().push(json!({"id":"ci-issue", "effect":"allow", "subjects":{"ids":["ci"]}, "actions":["credential.issue"], "resources":{"kinds":["*"], "ids":["other-ci"]}}));
	}).await;
	assert_eq!(status, 200, "{body}");
	let (status, issuer) = t
		.alice("POST", "/credentials", json!({"subject":"ci"}))
		.await;
	assert_eq!(status, 200, "{issuer}");
	let issuer_token = issuer["token"].as_str().unwrap();
	let (status, issued, _) =
		t.d.send(
			"POST",
			&t.path("/credentials"),
			None,
			None,
			Some(issuer_token),
			json!({"subject":"other-ci"}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let (status, child, _) =
		t.d.send(
			"POST",
			&t.path("/credentials"),
			None,
			None,
			Some(issuer_token),
			json!({"subject":"ci", "child":true, "expires_in_seconds":300}),
		)
		.await;
	assert_eq!(status, 200, "{child}");
	let (status, _, _) =
		t.d.send(
			"POST",
			&t.path("/credentials"),
			None,
			None,
			child["token"].as_str(),
			json!({"subject":"other-ci"}),
		)
		.await;
	assert_eq!(status, 403);
	let (status, revoked) = t
		.alice(
			"POST",
			&format!(
				"/credentials/{}/revoke",
				issuer["credential"]["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{revoked}");
	assert_eq!(
		t.bearer(
			issued["token"].as_str().unwrap(),
			"GET",
			"/api/state",
			Value::Null
		)
		.await,
		200
	);
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_child_credential_ends_with_its_parent_and_cannot_issue_children(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, parent) = t
		.bob(
			"POST",
			"/credentials",
			json!({"subject":"bob", "expires_in_seconds":3600}),
		)
		.await;
	assert_eq!(status, 200, "{parent}");
	let token = parent["token"].as_str().unwrap();
	for invalid in [
		json!({"subject":"alice", "child":true, "expires_in_seconds":300}),
		json!({"subject":"bob", "child":true, "expires_in_seconds":7200}),
	] {
		let (status, _, _) =
			t.d.send(
				"POST",
				&t.path("/credentials"),
				None,
				None,
				Some(token),
				invalid,
			)
			.await;
		assert!(status == 400 || status == 403, "unexpected status {status}");
	}
	let (status, child, _) =
		t.d.send(
			"POST",
			&t.path("/credentials"),
			None,
			None,
			Some(token),
			json!({"subject":"bob", "child":true, "expires_in_seconds":1800}),
		)
		.await;
	assert_eq!(status, 200, "{child}");
	assert_eq!(child["credential"]["parent_id"], parent["credential"]["id"]);
	let child_token = child["token"].as_str().unwrap();
	let (status, _, _) =
		t.d.send(
			"POST",
			&t.path("/credentials"),
			None,
			None,
			Some(child_token),
			json!({"subject":"bob", "child":true, "expires_in_seconds":300}),
		)
		.await;
	assert_eq!(status, 403);
	let (status, body) = t
		.bob(
			"POST",
			&format!(
				"/credentials/{}/revoke",
				parent["credential"]["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, parent_body, _) =
		t.d.send("GET", "/api/state", None, None, Some(token), Value::Null)
			.await;
	assert_eq!(status, 401, "{parent_body}");
	assert_eq!(parent_body["reason"], "revoked");
	let (status, body, _) =
		t.d.send(
			"GET",
			"/api/state",
			None,
			None,
			Some(child_token),
			Value::Null,
		)
		.await;
	assert_eq!(status, 401, "{body}");
	assert_eq!(body["reason"], "parent_ended");
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn tenant_credential_maximum_shortens_existing_credentials(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, body) = t
		.replace(|bundle| {
			bundle["subjects"]["ci"] =
				json!({"kind":"service", "node": t.node,"groups":["members"]});
		})
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, issued) = t
		.alice(
			"POST",
			"/credentials",
			json!({"subject":"ci", "expires_in_seconds": 60 * 86400}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let path = format!("/api/tenants/{}/credential-maximum", t.tenant);
	let (status, body) =
		t.d.as_person(
			&t.bob,
			"PUT",
			&path,
			Some(&t.bob_context),
			json!({"seconds":86400}),
		)
		.await;
	assert_eq!(status, 403, "{body}");
	let (status, body) =
		t.d.as_person(
			&t.alice,
			"PUT",
			&path,
			Some(&t.alice_context),
			json!({"seconds":86400}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, listed) = t.alice("GET", "/credentials", Value::Null).await;
	assert_eq!(status, 200, "{listed}");
	let row = listed
		.as_array()
		.unwrap()
		.iter()
		.find(|row| row["id"] == issued["credential"]["id"])
		.unwrap();
	assert_eq!(row["end_reason"], "lowered_maximum");
	let created =
		chrono::DateTime::parse_from_rfc3339(row["created_at"].as_str().unwrap()).unwrap();
	let expires =
		chrono::DateTime::parse_from_rfc3339(row["expires_at"].as_str().unwrap()).unwrap();
	assert_eq!((expires - created).num_seconds(), 86400);
	let (status, body) = t
		.alice(
			"POST",
			"/credentials",
			json!({"subject":"ci", "expires_in_seconds":86401}),
		)
		.await;
	assert_eq!(status, 400, "{body}");
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_maximum_shortens_credentials_issued_under_a_higher_deployment_maximum(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, body) = t
		.replace(|bundle| {
			bundle["subjects"]["ci"] =
				json!({"kind":"service", "node": t.node,"groups":["members"]});
		})
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, issued) = t
		.alice("POST", "/credentials", json!({"subject":"ci"}))
		.await;
	assert_eq!(status, 200, "{issued}");
	let id = Uuid::parse_str(issued["credential"]["id"].as_str().unwrap()).unwrap();
	// Issued while the Deployment allowed 200 days; the Deployment's 90 days is
	// already below it, so the Tenant's 90 days is no lower than what it reads.
	sqlx::query(
		"UPDATE authorization_credentials SET expires_at = created_at + interval '200 days' WHERE id = $1",
	)
	.bind(id)
	.execute(t.d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let (status, body) =
		t.d.as_person(
			&t.alice,
			"PUT",
			&format!("/api/tenants/{}/credential-maximum", t.tenant),
			Some(&t.alice_context),
			json!({"seconds": 90 * 86400}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, listed) = t.alice("GET", "/credentials", Value::Null).await;
	assert_eq!(status, 200, "{listed}");
	let row = listed
		.as_array()
		.unwrap()
		.iter()
		.find(|row| row["id"] == issued["credential"]["id"])
		.unwrap();
	assert_eq!(row["end_reason"], "lowered_maximum");
	let created =
		chrono::DateTime::parse_from_rfc3339(row["created_at"].as_str().unwrap()).unwrap();
	let expires =
		chrono::DateTime::parse_from_rfc3339(row["expires_at"].as_str().unwrap()).unwrap();
	assert_eq!((expires - created).num_seconds(), 90 * 86400);
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
			bundle["subjects"]["ci"] =
				json!({"kind":"service", "node": t.node,"groups":["members"]});
			// A person's subject stays theirs even after its kind changes.
			bundle["subjects"]["bob"]["kind"] = json!("service");
			bundle["subjects"]["bob"]["node"] = json!(t.node);
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
			bundle["subjects"]["ci"] =
				json!({"kind":"service", "node": t.node,"groups":["members"]});
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

/// A mapped subject's discovery of a Node in another Tenant of this Deployment
/// runs the receiver's scoped discovery in this process, under the sender's
/// own stack (#645). It completes on a debug build's worker stack whatever
/// `RUST_MIN_STACK` the test run sets.
#[rstest::rstest]
fn a_mapped_subject_discovers_another_tenants_node_on_the_worker_stack(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let discovery = async move {
		let t = Team::new(runtime).await;
		let (remote_person, remote_tenant, remote_context, remote) =
			t.d.nodes_tenant("Remote").await;
		let receiving =
			t.d.default_node(&t.alice, &t.alice_context, &t.tenant)
				.await;
		let address = t.d.address_of(&t.alice, &t.alice_context, &t.tenant).await;
		t.d.accepts(&t.alice, &t.alice_context, &t.tenant, &remote)
			.await;
		t.d.accepts(&remote_person, &remote_context, &remote_tenant, &address)
			.await;
		let (status, body) = t
			.replace(|bundle| {
				bundle["subjects"]["ci"] =
					json!({"kind":"service","node":receiving,"groups":["members"]});
				bundle["policies"].as_array_mut().unwrap().push(json!({
					"id":"ci-discovers","effect":"allow","subjects":{"ids":["ci"]},
					"actions":["federation.discover","agent.execute"],"resources":{"kinds":["*"]}
				}));
			})
			.await;
		assert_eq!(status, 200, "{body}");
		let (_, service) = t
			.alice("POST", "/credentials", json!({"subject":"ci"}))
			.await;
		let (status, body) = t
			.alice(
				"POST",
				"/peer-mappings",
				json!({"node":receiving,"source_node":remote,"source_subject":"agent",
					"credential_id":service["credential"]["id"],"enabled":true,
					"expected_revision":0}),
			)
			.await;
		assert_eq!(status, 200, "{body}");
		let sender = aidash_domain::federation::route::Sender {
			tenant: aidash_domain::tenancy::TenantName::new(&remote_tenant).unwrap(),
			node: None,
			subject: Some("agent".into()),
		};

		let discovered =
			t.d.federation
				.request::<Value>(
					&sender,
					&address,
					reqwest::Method::POST,
					"/scoped/discover",
					Some(&json!({"search":{}})),
				)
				.await;

		assert!(discovered.is_ok(), "{discovered:?}");
		t.finish().await;
	};
	std::thread::Builder::new()
		.stack_size(aidash_server::bootstrap::management::DEBUG_WORKER_STACK_BYTES)
		.spawn(move || {
			tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()
				.unwrap()
				.block_on(discovery)
		})
		.unwrap()
		.join()
		.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn overview_lists_item_scoped_credential_targets_without_policy_read(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, response) = t.replace(|bundle| {
  bundle["subjects"]["ci"] = json!({"kind":"service","node":t.node});
  bundle["subjects"]["other-ci"] = json!({"kind":"service","node":t.node});
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

#[rstest::rstest]
#[tokio::test]
async fn memory_scope_is_tenant_admin_only_and_revision_checked(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let team = Team::new(runtime).await;
	let path = format!("/api/tenants/{}/memory-scope", team.tenant);
	let (status, current) = team
		.d
		.as_person(
			&team.alice,
			"GET",
			&path,
			Some(&team.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{current}");
	assert_eq!(current, json!({"scope":"thread","revision":0}));
	let update = json!({"scope":"tenant_agent","expected_revision":0});
	let (status, denied) = team
		.d
		.as_person(
			&team.bob,
			"PUT",
			&path,
			Some(&team.bob_context),
			update.clone(),
		)
		.await;
	assert_eq!(status, 403, "{denied}");
	let (status, saved) = team
		.d
		.as_person(
			&team.alice,
			"PUT",
			&path,
			Some(&team.alice_context),
			update.clone(),
		)
		.await;
	assert_eq!(status, 200, "{saved}");
	assert_eq!(saved, json!({"scope":"tenant_agent","revision":1}));
	let (status, _) = team
		.d
		.as_person(&team.alice, "PUT", &path, Some(&team.alice_context), update)
		.await;
	assert_eq!(status, 409);
	let (status, restored) = team
		.d
		.as_person(
			&team.alice,
			"PUT",
			&path,
			Some(&team.alice_context),
			json!({"scope":"thread","expected_revision":1}),
		)
		.await;
	assert_eq!(status, 200, "{restored}");
	assert_eq!(restored, json!({"scope":"thread","revision":2}));
	let (status, _) = team
		.d
		.as_person(
			&team.alice,
			"GET",
			"/api/tenants/other/memory-scope",
			Some(&team.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 403);
	team.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn data_protection_settings_are_tenant_admin_only_and_apply_before_the_next_send(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	use aidash_application::provider_access::{Context, ProviderAccess, Source};
	use aidash_server::apps::identity::repositories::provider_credentials::AdmittedAccess;
	use reinhardt::db::orm::{Json, Model};
	let team = Team::new(runtime).await;
	let path = format!("/api/tenants/{}/data-protection", team.tenant);
	let send = async |method: &str, path: &str, body| {
		team.d
			.as_person(&team.alice, method, path, Some(&team.alice_context), body)
			.await
	};
	let (status, defaults) = send("GET", &path, Value::Null).await;
	assert_eq!(status, 200, "{defaults}");
	assert_eq!(
		defaults["policy"],
		json!({"level":"no_training","forbid_plan_connections":false,"forbid_keyless_endpoints":false})
	);
	let no_training = json!({"level":"no_training","forbid_plan_connections":false,"forbid_keyless_endpoints":true});
	let (status, denied) = team
		.d
		.as_person(
			&team.bob,
			"PUT",
			&path,
			Some(&team.bob_context),
			no_training.clone(),
		)
		.await;
	assert_eq!(status, 403, "{denied}");
	// A real registered Keyless model appears in the impact preview.
	team.d.federation.store.pool.atomic(&TEST_FIXTURE, async |tx| {
		let entry = aidash_server::apps::registry::models::Definition::build()
			.tenant(team.tenant.clone()).id("private-model").version("1.0.0")
			.kind(serde_json::from_value(json!("model"))?)
			.metadata(Json(json!({"id":"private-model","version":"1.0.0","kind":"model","name":{"en":"Private model"},"description":{"en":"Keyless policy fixture"},"config":{"provider":"openrouter","model_id":"vendor/model","endpoint":"https://keyless.example/v1","context_window":8192,"modalities":["text"],"cost":{}}}))).finish();
		aidash_server::apps::registry::models::Definition::objects().insert_with_executor(tx, &entry).await.map_err(reinhardt::core::exception::Error::from)?;
		Ok(())
	}).await.unwrap();
	let (status, preview) = send("POST", &format!("{path}/impact"), no_training.clone()).await;
	assert_eq!(status, 200, "{preview}");
	assert!(
		preview["destinations"]
			.as_array()
			.unwrap()
			.iter()
			.any(|item| item["id"] == "private-model")
	);
	let access = AdmittedAccess {
		store: team.d.federation.store.clone(),
	};
	let context = Context {
		tenant: team.tenant.clone(),
		..Default::default()
	};
	access
		.check(&context, "https://keyless.example/v1", &Source::Keyless)
		.await
		.unwrap();
	let (status, updated) = send("PUT", &path, no_training).await;
	assert_eq!(status, 200, "{updated}");
	assert!(
		access
			.check(&context, "https://keyless.example/v1", &Source::Keyless)
			.await
			.unwrap_err()
			.to_string()
			.contains("forbids Keyless endpoints")
	);
	// The former model-call switch is gone; an unknown field is refused like any
	// unknown JSON field.
	let (status, _) = send(
		"PUT",
		&path,
		json!({"level":"zero_data_retention","model_call_zdr":false}),
	)
	.await;
	assert_eq!(status, 422);
	let (status, _) = send("PUT", &path, defaults["policy"].clone()).await;
	assert_eq!(status, 200);
	access
		.check(&context, "https://keyless.example/v1", &Source::Keyless)
		.await
		.unwrap();
	let (status, _) = send("GET", "/api/tenants/other/data-protection", Value::Null).await;
	assert_eq!(status, 403);
	assert_eq!(
		team.d
			.count("SELECT count(*) FROM tenant_data_protection_history")
			.await,
		2
	);
	assert_eq!(team.d.count("SELECT count(*) FROM dashboard_administration_history WHERE action='tenant.data_protection.update'").await, 2);
	team.finish().await;
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
	let thread = store
		.create_thread(&scope, "alice", "Confidential plan")
		.await
		.unwrap();
	let task = store
		.create_task(
			&scope,
			thread.id,
			&aidash_domain::NewTask {
				task_model: None,
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
	let thread_path = format!("/api/threads/{}", thread.id);
	let routes: Vec<(&str, String, Value)> = vec![
		("GET", thread_path.clone(), Value::Null),
		("GET", "/api/tasks".into(), Value::Null),
		(
			"POST",
			format!("/api/tasks/{}/abandon", task.id),
			json!({"revision":task.revision,"reason":"administrator attempt"}),
		),
		("GET", format!("{thread_path}/message-history"), Value::Null),
		(
			"GET",
			format!("{thread_path}/attachments/{}", Uuid::new_v4()),
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
			"threads",
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
			&thread_path,
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
			&thread_path,
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

#[rstest::rstest]
#[tokio::test]
async fn invitation_issuer_without_registration_approval_must_choose_pending(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let (status, body) = t
		.replace(|bundle| {
			bundle["policies"].as_array_mut().unwrap().push(json!({
				"id":"member-invites", "effect":"allow", "subjects":{"ids":["bob"]},
				"actions":["invitation.create"], "resources":{"kinds":["invitation"]}
			}));
		})
		.await;
	assert_eq!(status, 200, "{body}");
	let path = format!("/api/tenants/{}/invitations", t.tenant);
	let (status, body) =
		t.d.as_person(&t.bob, "POST", &path, Some(&t.bob_context), json!({}))
			.await;
	assert_eq!(status, 201, "{body}");
	let holder = t.d.person("omitted-choice-holder").await;
	let (status, request) =
		t.d.as_person(
			&holder,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":t.tenant,"secret":body["secret"]}),
		)
		.await;
	assert_eq!(status, 200, "{request}");
	assert_eq!(request["status"], "pending");
	let (status, body) =
		t.d.as_person(
			&t.bob,
			"POST",
			&path,
			Some(&t.bob_context),
			json!({"join_directly":true}),
		)
		.await;
	assert_eq!(status, 403, "{body}");
	let (status, body) =
		t.d.as_person(
			&t.bob,
			"POST",
			&path,
			Some(&t.bob_context),
			json!({"join_directly":false}),
		)
		.await;
	assert_eq!(status, 201, "{body}");
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn invitation_waits_when_tenant_issuer_loses_approval_authority(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let holder = t.d.person("waiting-tenant-holder").await;
	let path = format!("/api/tenants/{}/invitations", t.tenant);
	let (status, invitation) =
		t.d.as_person(&t.alice, "POST", &path, Some(&t.alice_context), json!({}))
			.await;
	assert_eq!(status, 201, "{invitation}");
	let (status, body) = t
		.replace(|bundle| {
			// Bob remains an administrator when Alice loses approval authority.
			bundle["subjects"]["bob"]["groups"] = json!(["administrators"]);
			bundle["policies"].as_array_mut().unwrap().push(json!({
				"id":"no-registration-approval", "effect":"deny", "subjects":{"ids":["owner"]},
				"actions":["registration.approve"], "resources":{"kinds":["registration"]}
			}));
		})
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, registration) =
		t.d.as_person(
			&holder,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":t.tenant,"secret":invitation["secret"]}),
		)
		.await;
	assert_eq!(status, 200, "{registration}");
	assert_eq!(registration["status"], "pending");
	assert!(t.d.context_or_none(&holder, &t.tenant).await.is_none());
	let (status, pending) =
		t.d.as_person(
			&t.alice,
			"GET",
			&format!("/api/tenants/{}/registrations", t.tenant),
			Some(&t.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending[0]["id"], registration["id"]);
	assert_eq!(pending[0]["proposed_groups"], json!(["members"]));
	let notices: i64 = sqlx::query_scalar("SELECT count(*) FROM dashboard_administration_history WHERE action = 'invitation.direct_join' AND invitation_id = $1")
		.bind(Uuid::parse_str(invitation["id"].as_str().unwrap()).unwrap())
		.fetch_one(t.d.federation.store.pool.fixture_driver()).await.unwrap();
	assert_eq!(notices, 0);
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn invitation_uses_another_current_mapping_of_its_issuer(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let holder = t.d.person("alternate-issuer-holder").await;
	let path = format!("/api/tenants/{}/invitations", t.tenant);
	let (status, invitation) =
		t.d.as_person(&t.alice, "POST", &path, Some(&t.alice_context), json!({}))
			.await;
	assert_eq!(status, 201, "{invitation}");
	let (status, body) = t
		.replace(|bundle| {
			bundle["subjects"]["alternate-owner"] = bundle["subjects"]["owner"].clone();
		})
		.await;
	assert_eq!(status, 200, "{body}");

	// Give the same External Identity a second Mapping, then retire the one
	// used when the link was issued. Its other Mapping still may approve.
	let original = Uuid::parse_str(Team::mapping(&t.alice_context)).unwrap();
	let credential = Uuid::new_v4();
	let mapping = Uuid::new_v4();
	let pool = t.d.federation.store.pool.fixture_driver();
	sqlx::query("INSERT INTO authorization_credentials (id, tenant, subject, token_hash, created_at, expires_at, revoked_at, issued_by, mapping_id) SELECT $1, tenant, 'alternate-owner', $2, created_at, expires_at, NULL, issued_by, NULL FROM authorization_credentials WHERE id = (SELECT credential_id FROM dashboard_mappings WHERE id = $3)")
		.bind(credential).bind(Sha256::digest(Uuid::new_v4().as_bytes()).to_vec()).bind(original).execute(pool).await.unwrap();
	sqlx::query("INSERT INTO dashboard_mappings (id, identity_id, tenant, subject, credential_id, enabled, revision) SELECT $1, identity_id, tenant, 'alternate-owner', $2, true, 1 FROM dashboard_mappings WHERE id = $3")
		.bind(mapping).bind(credential).bind(original).execute(pool).await.unwrap();
	sqlx::query("UPDATE dashboard_mappings SET enabled = false WHERE id = $1")
		.bind(original)
		.execute(pool)
		.await
		.unwrap();
	let (status, registration) =
		t.d.as_person(
			&holder,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":t.tenant,"secret":invitation["secret"]}),
		)
		.await;
	assert_eq!(status, 200, "{registration}");
	assert_eq!(registration["status"], "approved");
	let recorded: i64 = sqlx::query_scalar("SELECT count(*) FROM dashboard_administration_history WHERE action = 'registration.approve' AND registration_id = $1 AND actor_identity_id = $2 AND actor_mapping_id = $3")
		.bind(Uuid::parse_str(registration["id"].as_str().unwrap()).unwrap()).bind(t.alice.identity).bind(mapping).fetch_one(pool).await.unwrap();
	assert_eq!(recorded, 1);
	// The allowed check of a direct join is kept with its approval.
	let allowed =
		t.d.count(&format!(
			"SELECT count(*) FROM authorization_decisions WHERE tenant = '{}' \
			 AND action = 'registration.approve' AND resource_id = '{}' \
			 AND (decision->>'allowed')::boolean",
			t.tenant,
			registration["id"].as_str().unwrap()
		))
		.await;
	assert_eq!(allowed, 1);
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn invitation_waits_when_proposed_group_is_no_longer_assignable(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let tenant_holder = t.d.person("unassignable-tenant-holder").await;
	let deployment_holder = t.d.person("unassignable-deployment-holder").await;
	let path = format!("/api/tenants/{}/invitations", t.tenant);
	let (status, first) =
		t.d.as_person(
			&t.alice,
			"POST",
			&path,
			Some(&t.alice_context),
			json!({"groups":["members"]}),
		)
		.await;
	assert_eq!(status, 201, "{first}");
	let (status, second) =
		t.d.as_deployment_administrator("POST", &path, json!({"groups":["members"]}))
			.await;
	assert_eq!(status, 201, "{second}");
	let (status, policy) = t
		.replace(|bundle| bundle["groups"]["members"]["assignable"] = json!(false))
		.await;
	assert_eq!(status, 200, "{policy}");
	for (holder, invitation) in [(&tenant_holder, &first), (&deployment_holder, &second)] {
		let link = json!({"tenant":t.tenant,"secret":invitation["secret"]});
		let (status, request) =
			t.d.as_person(
				holder,
				"POST",
				"/auth/invitations/redeem",
				None,
				link.clone(),
			)
			.await;
		assert_eq!(status, 200, "{request}");
		assert_eq!(request["status"], "pending");
		assert!(t.d.context_or_none(holder, &t.tenant).await.is_none());
		// The issuer's allowed check rolls back with the approval it was for.
		let allowed =
			t.d.count(&format!(
				"SELECT count(*) FROM authorization_decisions WHERE tenant = '{}' \
				 AND action = 'registration.approve' AND resource_id = '{}' \
				 AND (decision->>'allowed')::boolean",
				t.tenant,
				request["id"].as_str().unwrap()
			))
			.await;
		assert_eq!(allowed, 0);
		let (status, _) =
			t.d.as_person(holder, "POST", "/auth/invitations/redeem", None, link)
				.await;
		assert_eq!(status, 404);
	}
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn invitation_waits_when_member_admission_is_refused(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[values(false, true)] failing: bool,
) {
	/// Refuses member admission, or fails it as a payment provider outage would.
	struct RefuseMember {
		failing: bool,
	}
	#[async_trait::async_trait]
	impl aidash_application::usage::Admission for RefuseMember {
		async fn admit(
			&self,
			_: &str,
			work: aidash_application::usage::Work,
		) -> aidash_application::Result<aidash_application::usage::Verdict> {
			if work != aidash_application::usage::Work::Member {
				return Ok(aidash_application::usage::Verdict::Admit);
			}
			if self.failing {
				return Err(aidash_application::Error::External(
					"payment provider unavailable".into(),
				));
			}
			Ok(aidash_application::usage::Verdict::refuse("member_limit"))
		}
	}
	let mut t = Team::new(runtime).await;
	let holder = t.d.person("admission-refused-holder").await;
	let path = format!("/api/tenants/{}/invitations", t.tenant);
	let (status, invitation) =
		t.d.as_person(&t.alice, "POST", &path, Some(&t.alice_context), json!({}))
			.await;
	assert_eq!(status, 201, "{invitation}");
	t.d.federation.store.admission = Arc::new(RefuseMember { failing });
	t.d.app.context.set_singleton(t.d.federation.clone());
	let link = json!({"tenant":t.tenant,"secret":invitation["secret"]});
	let (status, request) =
		t.d.as_person(
			&holder,
			"POST",
			"/auth/invitations/redeem",
			None,
			link.clone(),
		)
		.await;
	assert_eq!(status, 200, "{request}");
	assert_eq!(request["status"], "pending");
	assert!(t.d.context_or_none(&holder, &t.tenant).await.is_none());
	let (status, _) =
		t.d.as_person(&holder, "POST", "/auth/invitations/redeem", None, link)
			.await;
	assert_eq!(status, 404);
	t.finish().await;
}

/// Tenant Instructions and Node Instructions (#1010): every member reads them,
/// only an administrator's browser session writes them, the revision guards
/// concurrent edits, and a Subject Credential never writes them, not even an
/// administrator's own.
#[rstest::rstest]
#[tokio::test]
async fn standing_instructions_are_written_only_by_administrators_in_a_browser(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let tenant_path = format!("/api/tenants/{}/instructions", t.tenant);
	let node_path = format!("/api/tenants/{}/nodes/{}/instructions", t.tenant, t.node);
	for path in [&tenant_path, &node_path] {
		let (status, empty) =
			t.d.as_person(&t.bob, "GET", path, Some(&t.bob_context), Value::Null)
				.await;
		assert_eq!(status, 200, "{path}: {empty}");
		assert_eq!(empty["text"], "");
		assert_eq!(empty["revision"], 0);
		let (status, denied) =
			t.d.as_person(
				&t.bob,
				"PUT",
				path,
				Some(&t.bob_context),
				json!({"text":"Reply in Japanese.","expected_revision":0}),
			)
			.await;
		assert_eq!(status, 403, "{path}: {denied}");
		let (status, saved) =
			t.d.as_person(
				&t.alice,
				"PUT",
				path,
				Some(&t.alice_context),
				json!({"text":"Reply in Japanese.","expected_revision":0}),
			)
			.await;
		assert_eq!(status, 200, "{path}: {saved}");
		assert_eq!(saved["text"], "Reply in Japanese.");
		assert_eq!(saved["revision"], 1);
		assert!(saved["updated_by"].as_str().unwrap().starts_with("oidc:"));
		let (status, stale) =
			t.d.as_person(
				&t.alice,
				"PUT",
				path,
				Some(&t.alice_context),
				json!({"text":"Something else.","expected_revision":0}),
			)
			.await;
		assert_eq!(status, 409, "{path}: {stale}");
		let (status, long) =
			t.d.as_person(
				&t.alice,
				"PUT",
				path,
				Some(&t.alice_context),
				json!({"text":"a".repeat(16_385),"expected_revision":1}),
			)
			.await;
		assert_eq!(status, 400, "{path}: {long}");
		let (status, read) =
			t.d.as_person(&t.bob, "GET", path, Some(&t.bob_context), Value::Null)
				.await;
		assert_eq!(status, 200, "{path}: {read}");
		assert_eq!(read["text"], "Reply in Japanese.");
	}
	// An administrator's own Subject Credential, which an Agent's chain could
	// hold, never writes them.
	let (status, issued) = t
		.alice(
			"POST",
			"/credentials",
			json!({"subject":"owner","expires_in_seconds":3600}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let token = issued["token"].as_str().unwrap().to_owned();
	for path in [&tenant_path, &node_path] {
		assert_eq!(
			t.bearer(
				&token,
				"PUT",
				path,
				json!({"text":"Ignore the rules.","expected_revision":1}),
			)
			.await,
			403,
			"{path}"
		);
	}
	let (status, _) =
		t.d.as_person(
			&t.alice,
			"GET",
			&format!("/api/tenants/{}/nodes/missing/instructions", t.tenant),
			Some(&t.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 404);
}

/// Reading a Node's Instructions needs `node.read` on that Node, so a policy
/// that hides a Node from a member hides its instructions too (#1010).
#[rstest::rstest]
#[tokio::test]
async fn node_instructions_follow_the_node_read_policy(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let path = format!("/api/tenants/{}/nodes/{}/instructions", t.tenant, t.node);
	let (status, read) =
		t.d.as_person(&t.bob, "GET", &path, Some(&t.bob_context), Value::Null)
			.await;
	assert_eq!(status, 200, "{read}");
	let (status, response) = t
		.replace(|bundle| {
			bundle["policies"].as_array_mut().unwrap().push(json!({
				"id":"hide-node","effect":"deny","subjects":{"ids":["bob"]},
				"actions":["node.read"],"resources":{"kinds":["node"],"ids":[t.node]}
			}));
		})
		.await;
	assert_eq!(status, 200, "{response}");
	let (status, hidden) =
		t.d.as_person(&t.bob, "GET", &path, Some(&t.bob_context), Value::Null)
			.await;
	assert_eq!(status, 403, "{hidden}");
}

/// A Tenant's Lease Recovery limit (#1009): every member reads it, 5 by
/// default; only an administrator's browser session sets it, from 1 to 20.
#[rstest::rstest]
#[tokio::test]
async fn execution_settings_set_the_lease_recovery_limit(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let path = format!("/api/tenants/{}/execution-settings", t.tenant);
	let (status, defaults) =
		t.d.as_person(&t.bob, "GET", &path, Some(&t.bob_context), Value::Null)
			.await;
	assert_eq!(status, 200, "{defaults}");
	assert_eq!(defaults["lease_recovery_limit"], 5);
	assert_eq!(defaults["revision"], 0);
	let (status, denied) =
		t.d.as_person(
			&t.bob,
			"PUT",
			&path,
			Some(&t.bob_context),
			json!({"lease_recovery_limit":2,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 403, "{denied}");
	for limit in [0, 21] {
		let (status, invalid) =
			t.d.as_person(
				&t.alice,
				"PUT",
				&path,
				Some(&t.alice_context),
				json!({"lease_recovery_limit":limit,"expected_revision":0}),
			)
			.await;
		assert_eq!(status, 400, "{limit}: {invalid}");
	}
	let (status, saved) =
		t.d.as_person(
			&t.alice,
			"PUT",
			&path,
			Some(&t.alice_context),
			json!({"lease_recovery_limit":2,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 200, "{saved}");
	assert_eq!(saved["lease_recovery_limit"], 2);
	assert_eq!(saved["revision"], 1);
	let (status, stale) =
		t.d.as_person(
			&t.alice,
			"PUT",
			&path,
			Some(&t.alice_context),
			json!({"lease_recovery_limit":3,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 409, "{stale}");
	let (status, read) =
		t.d.as_person(&t.bob, "GET", &path, Some(&t.bob_context), Value::Null)
			.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["lease_recovery_limit"], 2);
}

/// Budgets (#1012): a person sets their own, `budget.manage` sets any, only
/// from a browser session; each scope has one Budget and changes are
/// revision-checked.
#[rstest::rstest]
#[tokio::test]
async fn budgets_are_set_by_their_people_in_a_browser(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let path = format!("/api/tenants/{}/budgets", t.tenant);
	let person = |subject: &str| json!({"scope":{"kind":"person","subject":subject},"hard_limit_nanocredits":5_000_000_000_i64,"on_hard_limit":"sleep"});
	let tenant = json!({"scope":{"kind":"tenant"},"period":"monthly","soft_limit_nanocredits":1_000_000_000_i64,"hard_limit_nanocredits":2_000_000_000_i64,"recipients":["owner@example.com"]});
	// A member sets their own Budget, not another person's or the Tenant's.
	let (status, own) =
		t.d.as_person(&t.bob, "POST", &path, Some(&t.bob_context), person("bob"))
			.await;
	assert_eq!(status, 200, "{own}");
	assert_eq!(own["revision"], 1);
	assert_eq!(own["spent_nanocredits"], 0);
	assert_eq!(own["standing"], "under");
	for body in [person("owner"), tenant.clone()] {
		let (status, denied) =
			t.d.as_person(&t.bob, "POST", &path, Some(&t.bob_context), body)
				.await;
		assert_eq!(status, 403, "{denied}");
	}
	// An administrator sets the Tenant's; a scope holds one Budget.
	let (status, created) =
		t.d.as_person(
			&t.alice,
			"POST",
			&path,
			Some(&t.alice_context),
			tenant.clone(),
		)
		.await;
	assert_eq!(status, 200, "{created}");
	let (status, duplicate) =
		t.d.as_person(&t.alice, "POST", &path, Some(&t.alice_context), tenant)
			.await;
	assert_eq!(status, 409, "{duplicate}");
	let (status, invalid) =
		t.d.as_person(
			&t.alice,
			"POST",
			&path,
			Some(&t.alice_context),
			json!({"scope":{"kind":"node","node":t.node}}),
		)
		.await;
	assert_eq!(status, 400, "{invalid}");
	// Each person lists what they may change.
	let (_, mine) =
		t.d.as_person(&t.bob, "GET", &path, Some(&t.bob_context), Value::Null)
			.await;
	assert_eq!(mine.as_array().unwrap().len(), 1, "{mine}");
	let (_, all) =
		t.d.as_person(&t.alice, "GET", &path, Some(&t.alice_context), Value::Null)
			.await;
	assert_eq!(all.as_array().unwrap().len(), 2, "{all}");
	// Changes check the revision.
	let own_path = format!("{path}/{}", own["id"].as_str().unwrap());
	let mut change = person("bob");
	change["hard_limit_nanocredits"] = json!(9_000_000_000_i64);
	change["expected_revision"] = json!(1);
	let (status, changed) =
		t.d.as_person(
			&t.bob,
			"PUT",
			&own_path,
			Some(&t.bob_context),
			change.clone(),
		)
		.await;
	assert_eq!(status, 200, "{changed}");
	assert_eq!(changed["revision"], 2);
	let (status, stale) =
		t.d.as_person(&t.bob, "PUT", &own_path, Some(&t.bob_context), change)
			.await;
	assert_eq!(status, 409, "{stale}");
	// An administrator's Subject Credential, which an Agent's chain could
	// hold, never sets a Budget.
	let (status, issued) = t
		.alice(
			"POST",
			"/credentials",
			json!({"subject":"owner","expires_in_seconds":3600}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let token = issued["token"].as_str().unwrap().to_owned();
	assert_eq!(t.bearer(&token, "POST", &path, person("owner")).await, 403);
	// A limit an administrator put on a person is not theirs to lift.
	let (status, imposed) =
		t.d.as_person(&t.alice, "PUT", &own_path, Some(&t.alice_context), {
			let mut lower = person("bob");
			lower["hard_limit_nanocredits"] = json!(1_000_000_000_i64);
			lower["expected_revision"] = json!(2);
			lower
		})
		.await;
	assert_eq!(status, 200, "{imposed}");
	let mut lift = person("bob");
	lift["hard_limit_nanocredits"] = json!(99_000_000_000_i64);
	lift["expected_revision"] = json!(3);
	let (status, lifted) =
		t.d.as_person(&t.bob, "PUT", &own_path, Some(&t.bob_context), lift)
			.await;
	assert_eq!(status, 403, "{lifted}");
	let (status, _) =
		t.d.as_person(
			&t.bob,
			"DELETE",
			&own_path,
			Some(&t.bob_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 403);
	// Only someone who may change a Budget removes it.
	let tenant_path = format!("{path}/{}", created["id"].as_str().unwrap());
	let (status, _) =
		t.d.as_person(
			&t.bob,
			"DELETE",
			&tenant_path,
			Some(&t.bob_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 403);
	let (status, _) =
		t.d.as_person(
			&t.alice,
			"DELETE",
			&tenant_path,
			Some(&t.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 204);
}

async fn configurator_context(
	t: &Team,
	person: &Person,
	person_context: &str,
) -> aidash_server::tool::ToolContext {
	use aidash_application::registry::system::configurator::{ID, VERSION};
	use aidash_server::{
		federation::{Federation, Home},
		tool::ToolContext,
	};
	use reinhardt::Injectable;
	let f = Federation::inject(&t.d.app.context).await.unwrap();
	let (status, thread) =
		t.d.as_person(
			person,
			"POST",
			"/api/threads",
			Some(person_context),
			json!({"title":"Tenant configuration"}),
		)
		.await;
	assert_eq!(status, 200, "{thread}");
	let thread: Uuid = serde_json::from_value(thread["id"].clone()).unwrap();
	let (status, task) =
		t.d.as_person(
			person,
			"POST",
			&format!("/api/threads/{thread}/tasks"),
			Some(person_context),
			json!({"title":"Configuration","description":"Change team settings"}),
		)
		.await;
	assert_eq!(status, 200, "{task}");
	let task: Uuid = serde_json::from_value(task["id"].clone()).unwrap();
	let id = Uuid::new_v4();
	let home = format!("{}/{}", f.config.deployment, t.node);
	let executor = aidash_domain::agent_subject(&home, ID, VERSION);
	let mapping: Uuid = Team::mapping(person_context).parse().unwrap();
	let mut tx = f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query("SELECT set_config('aidash.input_ledger_worker','true',true)")
		.execute(&mut *tx)
		.await
		.unwrap();
	sqlx::query("INSERT INTO task_executions (id,task_id,thread_id,home_node,node,agent_id,agent_version,phase,control,context,pending,tenant) VALUES ($1,$2,$3,$4,$5,$6,$7,'TOOL_CALL','ACTIVE',$8,$9,$10)")
        .bind(id).bind(task).bind(thread).bind(home).bind(&t.node).bind(ID).bind(VERSION)
        .bind(common::context(json!({}))).bind(common::tool_pending(json!({}))).bind(&t.tenant).execute(&mut *tx).await.unwrap();
	sqlx::query("INSERT INTO authorization_execution (run_id,task_id,thread_id,tenant,credential_id,root_subject,subject_chain) SELECT $1,$2,$3,tenant,credential_id,subject,ARRAY[subject,$4] FROM dashboard_mappings WHERE id=$5")
        .bind(id).bind(task).bind(thread).bind(executor).bind(mapping).execute(&mut *tx).await.unwrap();
	sqlx::query("INSERT INTO dashboard_execution_origins (run_id,tenant,identity_id,mapping_id) SELECT $1,tenant,identity_id,id FROM dashboard_mappings WHERE id=$2")
        .bind(id).bind(mapping).execute(&mut *tx).await.unwrap();
	sqlx::query("INSERT INTO conversations (id,tenant,thread_id,scratch_space_id,target,target_kind,created_by) SELECT $1,tenant,id,scratch_space_id,$2,'agent',created_by FROM threads WHERE id=$3")
		.bind(Uuid::new_v4()).bind(format!("{ID}@{VERSION}")).bind(thread).execute(&mut *tx).await.unwrap();
	tx.commit().await.unwrap();
	let scope = aidash_application::tenancy::TenancyScope::tenant(&t.tenant).unwrap();
	let run = f.store.run(&scope, id).await.unwrap();
	ToolContext {
		home: Home::new(f.clone(), run.clone()),
		store: f.store.clone(),
		run,
		scope,
	}
}

/// Exercise the worker boundary after admission, using its persisted grant
/// and browser mapping. The test does not require inference or a provider key.
#[rstest::rstest]
#[tokio::test]
async fn configurator_typed_tools_act_through_the_root_mapping(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	use aidash_application::registry::system::configurator::{ID, VERSION};
	use aidash_server::tool::builtins;
	let t = Team::new(runtime).await;
	let (status, refused) = t.d.as_person(
		&t.bob, "POST", "/api/threads", Some(&t.bob_context),
		json!({"title":"Settings","request":"Configure this Tenant","target":{"id":ID,"version":VERSION},"target_kind":"agent"}),
	).await;
	assert_eq!(status, 403, "{refused}");
	assert_eq!(
		refused["code"],
		"tenant_configurator_administrator_required"
	);
	let context = configurator_context(&t, &t.alice, &t.alice_context).await;
	let thread = context.run.thread_id;
	let tools = builtins();
	let updated = tools["tenant_instructions_set"]
		.invoke(
			&context,
			json!({"text":"Prepare drafts only.","expected_revision":0}),
			"instructions",
		)
		.await
		.unwrap();
	assert_eq!(updated["revision"], 1, "Instructions response: {updated}");
	assert!(
		updated["updated_by"]
			.as_str()
			.unwrap()
			.contains(&thread.to_string())
	);
	let protection = tools["data_protection_set"]
		.invoke(
			&context,
			json!({"level":"no_training","forbid_keyless_endpoints":true}),
			"protection",
		)
		.await
		.unwrap();
	assert_eq!(
		protection["policy"]["forbid_keyless_endpoints"], true,
		"{protection}"
	);
	let lowered = tools["data_protection_set"]
		.invoke(
			&context,
			json!({"level":"no_restriction","allow_plan_connections":true}),
			"lower",
		)
		.await
		.unwrap();
	assert_eq!(
		lowered["policy"],
		json!({"level":"no_restriction","forbid_keyless_endpoints":true,"forbid_plan_connections":false})
	);
	assert_eq!(lowered["previous"], protection["policy"]);
	assert_eq!(lowered["impact"]["effective"], lowered["policy"]);
	assert_eq!(lowered["revision"], 2);
	let (stale, _) =
		t.d.as_person(
			&t.alice,
			"PUT",
			&format!("/api/tenants/{}/data-protection", t.tenant),
			Some(&t.alice_context),
			json!({"level":"no_training","expected_revision":1}),
		)
		.await;
	assert_eq!(stale, 409);
	let (status, read) =
		t.d.as_person(
			&t.alice,
			"GET",
			&format!("/api/tenants/{}/instructions", t.tenant),
			Some(&t.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["text"], "Prepare drafts only.");
	for path in [
		format!("/api/tenants/{}/purge", t.tenant),
		format!("/api/tenants/{}/nodes/{}/purge", t.tenant, t.node),
		"/api/dashboard/registrations".into(),
	] {
		assert!(
			tools["aidash_api"]
				.invoke(&context, json!({"method":"POST","path":path}), "refused")
				.await
				.is_err()
		);
	}
	for mode in ["manual", "bypass"] {
		let (status, changed) =
			t.d.as_person(
				&t.alice,
				"PUT",
				&format!("/api/threads/{thread}/approval-mode"),
				Some(&t.alice_context),
				json!({"mode":mode}),
			)
			.await;
		assert_eq!(status, 200, "switch to {mode}: {changed}");
		assert_eq!(changed["mode"], mode);
	}
	let catalog = tools["aidash_api_catalog"]
		.invoke(
			&context,
			json!({"query":"POST registrations/{id}/approve"}),
			"registration-schema",
		)
		.await
		.unwrap();
	let operations = catalog["operations"].as_array().unwrap();
	assert_eq!(operations.len(), 1, "{catalog}");
	assert_eq!(
		operations[0]["path"],
		"/api/tenants/{tenant}/registrations/{id}/approve"
	);
	assert_eq!(
		operations[0]["request"]["properties"]["subject"]["type"],
		"string"
	);
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn configurator_approval_covers_agent_and_tool(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let t = Team::new(runtime).await;
	let context = configurator_context(&t, &t.alice, &t.alice_context).await;
	let tools = aidash_server::tool::builtins();
	let node = t.d.federation.config.deployment.as_str();
	let entry = |kind: &str, id: &str, config: Value| json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Team test definition"},"config":config});
	let tool = entry(
		"tool",
		"review-tool",
		json!({"registry_deployment":node,"provider":"integration.http@1","operation":"invoke","default_alias":"review_tool","tier":"integration","transport":{"transport":"http","endpoint":"https://tools.example.com/lookup","replay":"read_only"}}),
	);
	let agent = entry(
		"agent",
		"review-agent",
		json!({"schema_version":1,"instructions":"Use the review tool.","bindings":[{"kind":"tool","target":{"registry_deployment":node,"id":"review-tool","version":"1.0.0"},"alias":"review_tool"}]}),
	);
	for definition in [tool, agent] {
		tools["registry_create"]
			.invoke(&context, definition, "register")
			.await
			.unwrap();
	}
	let input = json!({"node":t.node,"entry":{"id":"review-agent","version":"1.0.0"}});
	let member = configurator_context(&t, &t.bob, &t.bob_context).await;
	assert!(
		tools["catalog_review_approve"]
			.invoke(&member, input.clone(), "refused")
			.await
			.is_err()
	);
	let approved = tools["catalog_review_approve"]
		.invoke(&context, input.clone(), "approve")
		.await
		.unwrap();
	assert_eq!(approved["ready"], true, "{approved}");
	assert_eq!(approved["blocked"], json!([]));
	assert_eq!(
		approved["approved"]["bindings"].as_array().unwrap().len(),
		2,
		"{approved}"
	);
	let (status, review) = t
		.alice(
			"GET",
			&format!("/catalog/review?node={}&entry=review-agent@1.0.0", t.node),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{review}");
	for id in ["review-agent", "review-tool"] {
		let item = review["items"]
			.as_array()
			.unwrap()
			.iter()
			.find(|v| v["reference"]["id"] == id)
			.unwrap();
		assert_eq!(item["status"], "approved", "{item}");
		assert_eq!(item["scope"], "deployment", "{item}");
	}
	let repeat = tools["catalog_review_approve"]
		.invoke(&context, input, "again")
		.await
		.unwrap();
	assert_eq!(repeat["approved"]["bindings"], json!([]));
	t.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn configurator_reports_data_protection_floor(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	use futures_util::FutureExt;
	let t = Team::new(
		runtime
			.map(|mut r| {
				r.federation.store.data_protection_floor =
					aidash_domain::data_protection::Level::NoTraining;
				r
			})
			.boxed()
			.shared(),
	)
	.await;
	let context = configurator_context(&t, &t.alice, &t.alice_context).await;
	let error = aidash_server::tool::builtins()["data_protection_set"]
		.invoke(&context, json!({"level":"no_restriction"}), "lower")
		.await
		.unwrap_err();
	assert!(
		error.to_string().contains("Deployment floor NoTraining"),
		"{error}"
	);
	let (status, saved) =
		t.d.as_person(
			&t.alice,
			"GET",
			&format!("/api/tenants/{}/data-protection", t.tenant),
			Some(&t.alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200);
	assert_eq!(saved["policy"]["level"], "no_training");
	assert_eq!(saved["revision"], 0);
	t.finish().await;
}
