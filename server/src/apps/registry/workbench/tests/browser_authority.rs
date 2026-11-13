//! Recheck browser authority after authentication and before a native mutation.
use super::{Workbench, captured_actor_application, common, request, workbench};
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::apps::identity::models::{
	DashboardIdentity, DashboardMapping, DashboardSession,
};
use aidash_server::authorization::Authorization;
use aidash_server::config::OidcConfig;
use chrono::{Duration, Utc};
use http::{HeaderMap, Method};
use reinhardt::db::orm::Model;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[rstest::rstest]
#[tokio::test]
async fn fixture_only_call_needs_no_identifiable_starter(#[future(awt)] workbench: Workbench) {
	use std::collections::VecDeque;
	let mut wb = workbench;
	let tool = json!({"id":"simulated-unsafe","version":"1.0.0","kind":"tool","name":{"en":"Simulated"},"description":{"en":"Fixture"},"config":{"registry_deployment":wb.f.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"approval_tool","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":format!("{}/effect",wb.endpoint),"replay":"unsafe"}}});
	wb.f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			serde_json::from_value(tool).unwrap(),
		)
		.await
		.unwrap();
	let mut entry = wb.draft["entry"].clone();
	entry["config"]["bindings"] = json!([{"kind":"tool","target":{"registry_deployment":wb.f.config.deployment,"id":"simulated-unsafe","version":"1.0.0"},"alias":"approval_tool","narrow":{}}]);
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":entry,"documents":[]}),
		)
		.await;
	*wb.responses.lock().await = VecDeque::from([
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[{"id":"call-1","function":{"name":"approval_tool","arguments":"{}"}}]}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Complete"}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
	]);
	let result = wb.finished(json!({"expected_revision":2,"message":"Use the tool","fixtures":{"approval_tool":{"status":"success","response":{"ok":true}}}})).await;
	assert_eq!(result["status"], "completed", "{result}");
	assert_eq!(result["tool_calls"][0]["outcome"], "simulated");
	assert!(result["scenario"].get("pending_approval").is_none());
	wb.cleanup().await;
}

#[rstest::rstest]
#[case::allow_once("allow_once", "completed")]
#[case::deny("deny", "blocked")]
#[case::not_eligible("ineligible", "blocked")]
#[case::eligibility_revoked("revoked", "blocked")]
#[case::test_permission_revoked("test_revoked", "blocked")]
#[case::second_call_requires_new_decision("twice", "blocked")]
#[case::changed_arguments("tampered", "blocked")]
#[tokio::test]
async fn test_approval_belongs_to_its_starter_and_current_policy(
	#[case] choice: &str,
	#[case] expected: &str,
	#[future(awt)] browser_workbench: (Workbench, HeaderMap, DashboardSession),
) {
	use std::{collections::VecDeque, time::Duration};
	let (mut wb, headers, _) = browser_workbench;
	let grants_before: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM core_records WHERE tenant = 'acme' AND kind = 'grant'",
	)
	.fetch_one(wb.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let tool = json!({"id":"approval-test-tool","version":"1.0.0","kind":"tool","name":{"en":"Approval fixture"},"description":{"en":"Fixture"},"config":{"registry_deployment":wb.f.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"approval_tool","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":format!("{}/effect",wb.endpoint),"replay":"unsafe"}}});
	wb.f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			serde_json::from_value(tool).unwrap(),
		)
		.await
		.unwrap();
	let mut entry = wb.draft["entry"].clone();
	entry["config"]["bindings"] = json!([{"kind":"tool","target":{"registry_deployment":wb.f.config.deployment,"id":"approval-test-tool","version":"1.0.0"},"alias":"approval_tool","narrow":{}}]);
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":entry,"documents":[]}),
		)
		.await;
	assert_eq!(request(&wb.app, &wb.token, "PUT", "/api/workbench/test-profiles/acme/sandbox", json!({"expected_revision":0,"enabled":true,"rules":[{"tool":{"id":"approval-test-tool","version":"1.0.0"},"endpoint":format!("{}/test-effect",wb.endpoint),"read_only_verified":true,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]})).await.0, 200);
	// Neither the person's Don't ask mode nor a deny rule covers Workbench tests.
	sqlx::query("INSERT INTO identity_approval_preferences (tenant, subject, default_mode, revision, updated_at) VALUES ('acme', 'alice', 'dont_ask', 1, now())").execute(wb.f.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO identity_approval_rules (tenant, id, subject, kind, target, effect, created_at) VALUES ('acme', $1, 'alice', 'tool_call', $2, 'deny', now())").bind(Uuid::new_v4()).bind(json!("every")).execute(wb.f.store.pool.fixture_driver()).await.unwrap();
	if choice == "ineligible" {
		let revision = wb.policy_revision().await;
		let bundle = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"no-approval","effect":"deny","subjects":{"ids":["alice"]},"actions":["capability.approve"],"resources":{"kinds":["tool"]}}]});
		assert_eq!(
			request(
				&wb.app,
				&wb.f.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":revision,"bundle":bundle})
			)
			.await
			.0,
			200
		);
	}
	*wb.responses.lock().await = VecDeque::from([
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[{"id":"call-1","function":{"name":"approval_tool","arguments":"{\"action\":\"read\",\"resource\":\"sandbox\"}"}}]}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Complete"}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
	]);
	if choice == "twice" {
		wb.responses.lock().await[0]["choices"][0]["message"]["tool_calls"] = json!([
			{"id":"call-1","function":{"name":"approval_tool","arguments":"{\"action\":\"read\",\"resource\":\"sandbox\"}"}},
			{"id":"call-2","function":{"name":"approval_tool","arguments":"{\"action\":\"read\",\"resource\":\"sandbox\",\"extra\":\"changed\"}"}}
		]);
	}
	let (actor, _) =
		aidash_server::dashboard_auth::actor_from_headers(&wb.f, &headers, &Method::GET)
			.await
			.unwrap();
	let app = common::application_with(wb.f.clone(), |_| {
		use aidash_server::apps::registry::workbench::views::test;
		reinhardt::ServerRouter::new()
			.endpoint(test::start)
			.endpoint(test::sessions)
			.endpoint(test::answer_approval)
			.with_middleware(super::CapturedActor(actor.clone()))
	})
	.await;
	let other_actor = other_person_same_subject(&wb).await;
	let other = common::application_with(wb.f.clone(), |_| {
		use aidash_server::apps::registry::workbench::views::test;
		reinhardt::ServerRouter::new()
			.endpoint(test::sessions)
			.endpoint(test::answer_approval)
			.with_middleware(super::CapturedActor(other_actor.clone()))
	})
	.await;
	let path = format!("{}/tests", wb.path());
	let (status, started) = request(
		&app,
		&wb.token,
		"POST",
		&path,
		json!({"expected_revision":2,"message":"Use the tool","mode":"real","profile_id":"sandbox"}),
	)
	.await;
	assert_eq!(status, 200, "{started}");
	let id = started["id"].as_str().unwrap();
	let mut answered = false;
	let mut first_approval = Value::Null;
	let mut second_answered = false;
	let mut revoked = false;
	let mut finished = Value::Null;
	for _ in 0..1200 {
		let (status, sessions) = request(&app, &wb.token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{sessions}");
		let session = sessions
			.as_array()
			.unwrap()
			.iter()
			.find(|s| s["id"] == id)
			.unwrap();
		if choice == "twice"
			&& answered
			&& !second_answered
			&& session["scenario"]["pending_approval"]["state"] == "pending"
		{
			let pending = &session["scenario"]["pending_approval"];
			assert_ne!(pending["id"], first_approval);
			assert_eq!(pending["call"]["arguments"]["extra"], "changed");
			assert_eq!(wb.effect_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
			let endpoint = format!("/api/workbench/tests/{id}/approval");
			assert_eq!(
				request(
					&app,
					&wb.token,
					"POST",
					&endpoint,
					json!({"approval_id":first_approval,"choice":"allow_once"})
				)
				.await
				.0,
				403
			);
			assert_eq!(
				request(
					&app,
					&wb.token,
					"POST",
					&endpoint,
					json!({"approval_id":pending["id"],"choice":"deny"})
				)
				.await
				.0,
				200
			);
			second_answered = true;
		}
		if session["scenario"]["pending_approval"]["state"] == "pending" && !answered && !revoked {
			assert_ne!(choice, "ineligible");
			let (status, others) = request(&other, &wb.token, "GET", &path, Value::Null).await;
			assert_eq!(status, 200, "{others}");
			assert!(others[0]["scenario"].get("pending_approval").is_none());
			let other_answer = json!({"approval_id":session["scenario"]["pending_approval"]["id"],"choice":"allow_once"});
			assert_eq!(
				request(
					&other,
					&wb.token,
					"POST",
					&format!("/api/workbench/tests/{id}/approval"),
					other_answer
				)
				.await
				.0,
				403
			);
			if choice == "test_revoked" {
				let approval_id = session["scenario"]["pending_approval"]["id"].clone();
				let revision = wb.policy_revision().await;
				let bundle = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"no-test","effect":"deny","subjects":{"ids":["alice"]},"actions":["agent_draft.test"],"resources":{"kinds":["agent_draft"]}}]});
				assert_eq!(
					request(
						&wb.app,
						&wb.f.config.api_token,
						"POST",
						"/api/authorization/acme",
						json!({"expected_revision":revision,"bundle":bundle})
					)
					.await
					.0,
					200
				);
				let (_, hidden) = request(&app, &wb.token, "GET", &path, Value::Null).await;
				assert!(hidden[0]["scenario"].get("pending_approval").is_none());
				let endpoint = format!("/api/workbench/tests/{id}/approval");
				assert_eq!(
					request(
						&app,
						&wb.token,
						"POST",
						&endpoint,
						json!({"approval_id":approval_id,"choice":"deny"})
					)
					.await
					.0,
					403
				);
				let revision = wb.policy_revision().await;
				let bundle = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]});
				assert_eq!(
					request(
						&wb.app,
						&wb.f.config.api_token,
						"POST",
						"/api/authorization/acme",
						json!({"expected_revision":revision,"bundle":bundle})
					)
					.await
					.0,
					200
				);
				assert_eq!(
					request(
						&app,
						&wb.token,
						"POST",
						&endpoint,
						json!({"approval_id":approval_id,"choice":"deny"})
					)
					.await
					.0,
					200
				);
				answered = true;
				continue;
			}
			if choice == "revoked" {
				let revision = wb.policy_revision().await;
				let bundle = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"no-approval","effect":"deny","subjects":{"ids":["alice"]},"actions":["capability.approve"],"resources":{"kinds":["tool"]}}]});
				assert_eq!(
					request(
						&wb.app,
						&wb.f.config.api_token,
						"POST",
						"/api/authorization/acme",
						json!({"expected_revision":revision,"bundle":bundle})
					)
					.await
					.0,
					200
				);
				let (_, hidden) = request(&app, &wb.token, "GET", &path, Value::Null).await;
				assert!(hidden[0]["scenario"].get("pending_approval").is_none());
				revoked = true;
				continue;
			}
			first_approval = session["scenario"]["pending_approval"]["id"].clone();
			if choice == "tampered" {
				let mut pending = session["scenario"]["pending_approval"].clone();
				pending["call"]["arguments"]["extra"] = json!("changed");
				sqlx::query("UPDATE agent_test_sessions SET scenario = jsonb_set(scenario, '{pending_approval}', $1) WHERE id = $2")
					.bind(pending).bind(Uuid::parse_str(id).unwrap()).execute(wb.f.store.pool.fixture_driver()).await.unwrap();
			}
			let answer = json!({"approval_id":first_approval,"choice":if choice == "twice" || choice == "tampered" { "allow_once" } else { choice }});
			let endpoint = format!("/api/workbench/tests/{id}/approval");
			assert_eq!(
				request(&wb.app, &wb.token, "POST", &endpoint, answer.clone())
					.await
					.0,
				403,
				"a subject token is not the person who started the test"
			);
			assert_eq!(
				request(&app, &wb.token, "POST", &endpoint, answer.clone())
					.await
					.0,
				200
			);
			assert_eq!(
				request(&app, &wb.token, "POST", &endpoint, answer).await.0,
				403
			);
			answered = true;
		}
		if session["status"] != "running" {
			finished = session.clone();
			break;
		}
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
	assert_eq!(finished["status"], expected, "{finished}");
	assert_eq!(
		answered,
		matches!(
			choice,
			"allow_once" | "deny" | "test_revoked" | "twice" | "tampered"
		)
	);
	assert_eq!(second_answered, choice == "twice");
	if choice == "tampered" {
		assert_eq!(wb.effect_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
	}
	assert_eq!(revoked, choice == "revoked");
	assert_eq!(
		finished["tool_calls"][0]["outcome"],
		if choice == "allow_once" || choice == "twice" {
			"real"
		} else {
			"denied"
		}
	);
	let grants_after: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM core_records WHERE tenant = 'acme' AND kind = 'grant'",
	)
	.fetch_one(wb.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(grants_after, grants_before);
	wb.cleanup().await;
}

/// A call that needs an approval has nobody to ask under an API token, or in a
/// session that recorded no starter. It is denied and no request is written.
#[rstest::rstest]
#[case::api_token(false)]
#[case::no_starter_recorded(true)]
#[tokio::test]
async fn a_call_needing_approval_without_a_starter_is_denied_unasked(
	#[case] browser: bool,
	#[future(awt)] browser_workbench: (Workbench, HeaderMap, DashboardSession),
) {
	use std::{collections::VecDeque, sync::atomic::Ordering, time::Duration};
	let (mut wb, headers, _) = browser_workbench;
	let tool = json!({"id":"approval-test-tool","version":"1.0.0","kind":"tool","name":{"en":"Approval fixture"},"description":{"en":"Fixture"},"config":{"registry_deployment":wb.f.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"approval_tool","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":format!("{}/effect",wb.endpoint),"replay":"unsafe"}}});
	wb.f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			serde_json::from_value(tool).unwrap(),
		)
		.await
		.unwrap();
	let mut entry = wb.draft["entry"].clone();
	entry["config"]["bindings"] = json!([{"kind":"tool","target":{"registry_deployment":wb.f.config.deployment,"id":"approval-test-tool","version":"1.0.0"},"alias":"approval_tool","narrow":{}}]);
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":entry,"documents":[]}),
		)
		.await;
	assert_eq!(request(&wb.app, &wb.token, "PUT", "/api/workbench/test-profiles/acme/sandbox", json!({"expected_revision":0,"enabled":true,"rules":[{"tool":{"id":"approval-test-tool","version":"1.0.0"},"endpoint":format!("{}/test-effect",wb.endpoint),"read_only_verified":true,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]})).await.0, 200);
	*wb.responses.lock().await = VecDeque::from([
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[{"id":"call-1","function":{"name":"approval_tool","arguments":"{\"action\":\"read\",\"resource\":\"sandbox\"}"}}]}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Complete"}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
	]);
	let path = format!("{}/tests", wb.path());
	let input = json!({"expected_revision":2,"message":"Use the tool","mode":"real","profile_id":"sandbox"});
	let scenario = async |id: Uuid| -> Value {
		sqlx::query_scalar("SELECT scenario FROM agent_test_sessions WHERE id = $1")
			.bind(id)
			.fetch_one(wb.f.store.pool.fixture_driver())
			.await
			.unwrap()
	};
	let id = if browser {
		// The person starts the test; the session then loses its starter, as
		// a session recorded before starters were would have none.
		let (actor, _) =
			aidash_server::dashboard_auth::actor_from_headers(&wb.f, &headers, &Method::GET)
				.await
				.unwrap();
		let app = common::application_with(wb.f.clone(), |_| {
			use aidash_server::apps::registry::workbench::views::test;
			reinhardt::ServerRouter::new()
				.endpoint(test::start)
				.with_middleware(super::CapturedActor(actor.clone()))
		})
		.await;
		wb.model_gate.paused.store(true, Ordering::SeqCst);
		let (status, started) = request(&app, &wb.token, "POST", &path, input).await;
		assert_eq!(status, 200, "{started}");
		tokio::time::timeout(Duration::from_secs(30), wb.model_gate.arrived.notified())
			.await
			.unwrap();
		let id = Uuid::parse_str(started["id"].as_str().unwrap()).unwrap();
		assert!(scenario(id).await["starter_identity"].is_string());
		sqlx::query("UPDATE agent_test_sessions SET scenario = scenario - 'starter_identity' - 'starter_subject' WHERE id = $1")
			.bind(id)
			.execute(wb.f.store.pool.fixture_driver())
			.await
			.unwrap();
		wb.model_gate.paused.store(false, Ordering::SeqCst);
		wb.model_gate.release.notify_one();
		id
	} else {
		let started = wb.call("POST", &path, input).await;
		Uuid::parse_str(started["id"].as_str().unwrap()).unwrap()
	};
	let mut finished = Value::Null;
	for _ in 0..400 {
		let sessions = wb.call("GET", &path, Value::Null).await;
		let session = sessions
			.as_array()
			.unwrap()
			.iter()
			.find(|s| s["id"] == json!(id))
			.unwrap();
		if session["status"] != "running" {
			finished = session.clone();
			break;
		}
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
	assert_eq!(finished["status"], "blocked", "{finished}");
	assert_eq!(finished["tool_calls"][0]["outcome"], "denied");
	assert_eq!(finished["error"], "test approval was refused");
	let stored = scenario(id).await;
	assert!(stored.get("pending_approval").is_none(), "{stored}");
	assert!(stored["starter_identity"].is_null());
	assert_eq!(wb.effect_hits.load(Ordering::SeqCst), 0);
	wb.cleanup().await;
}

async fn other_person_same_subject(
	wb: &Workbench,
) -> aidash_server::authorization::identity::Actor {
	let now = Utc::now();
	let lease = reinhardt::db::orm::DatabaseConnectionLease::register(wb.f.store.pool.connection())
		.unwrap();
	let mut db = lease.handle();
	let identity = DashboardIdentity::build()
		.id(Uuid::new_v4())
		.issuer("https://accounts.google.com")
		.subject("other-browser-fixture")
		.valid_since(None)
		.verified_email(None)
		.display_name(None)
		.last_valid_at(Some(now))
		.disabled_at(None)
		.display_observed_at(None)
		.display_erased_at(None)
		.display_erased_by(None)
		.finish();
	DashboardIdentity::objects()
		.create_with_conn(&mut db, &identity)
		.await
		.unwrap();
	let session = DashboardSession::build()
		.id(Uuid::new_v4())
		.token_hash(common::browser_session_digest(b"other-workbench-session"))
		.csrf_hash(Sha256::digest(b"other-workbench-csrf").to_vec())
		.identity(identity.id)
		.provider_sid(None)
		.created_at(now)
		.auth_time(None)
		.last_activity_at(now)
		.expires_at(now + Duration::hours(12))
		.revoked_at(None)
		.desktop(false)
		.desktop_idle_seconds(None)
		.access_expires_at(None)
		.finish();
	DashboardSession::objects()
		.create_with_conn(&mut db, &session)
		.await
		.unwrap();
	let issued = Authorization {
		pool: wb.f.store.pool.clone(),
	}
	.issue_credential("acme", "alice", 3600, "deployment_administrator")
	.await
	.unwrap();
	let mapping = DashboardMapping::build()
		.id(Uuid::new_v4())
		.identity(identity.id)
		.tenant("acme")
		.subject("alice")
		.credential(issued.credential.id)
		.enabled(true)
		.revision(1)
		.finish();
	wb.f.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			DashboardMapping::objects()
				.create_with_conn(tx, &mapping)
				.await?;
			Ok(())
		})
		.await
		.unwrap();
	let mut headers = HeaderMap::new();
	headers.insert(
		"cookie",
		"__Host-aidash-session=other-workbench-session"
			.parse()
			.unwrap(),
	);
	headers.insert("origin", "https://localhost:8080".parse().unwrap());
	headers.insert("x-aidash-csrf", "other-workbench-csrf".parse().unwrap());
	headers.insert(
		"x-aidash-context",
		format!("mapping:{}:{}", mapping.tenant, mapping.id)
			.parse()
			.unwrap(),
	);
	aidash_server::dashboard_auth::actor_from_headers(&wb.f, &headers, &Method::GET)
		.await
		.unwrap()
		.0
}

#[rstest::fixture]
async fn browser_workbench(
	#[default("https://accounts.google.com")] issuer: &str,
	#[future(awt)] workbench: Workbench,
) -> (Workbench, HeaderMap, DashboardSession) {
	let mut wb = workbench;
	wb.f.config.oidc = Some(OidcConfig {
		issuer: issuer.into(),
		client_id: "aidash".into(),
		client_secret: "local-test-secret".into(),
		public_origin: "https://localhost:8080".into(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	let lease = reinhardt::db::orm::DatabaseConnectionLease::register(wb.f.store.pool.connection())
		.unwrap();
	let mut db = lease.handle();
	let now = Utc::now();
	let identity = DashboardIdentity::build()
		.id(Uuid::new_v4())
		.issuer(issuer)
		.subject("browser-fixture")
		.valid_since(None)
		.verified_email(None)
		.display_name(None)
		.last_valid_at(Some(now))
		.disabled_at(None)
		.approval_request_mail(true)
		.display_observed_at(None)
		.display_erased_at(None)
		.display_erased_by(None)
		.finish();
	DashboardIdentity::objects()
		.create_with_conn(&mut db, &identity)
		.await
		.unwrap();
	let session = DashboardSession::build()
		.id(Uuid::new_v4())
		.token_hash(common::browser_session_digest(b"browser-authority-session"))
		.csrf_hash(Sha256::digest(b"browser-authority-csrf").to_vec())
		.identity(identity.id)
		.provider_sid(None)
		.created_at(now)
		.auth_time(None)
		.last_activity_at(now)
		.expires_at(now + Duration::hours(12))
		.revoked_at(None)
		.desktop(false)
		.desktop_idle_seconds(None)
		.access_expires_at(None)
		.finish();
	let session = DashboardSession::objects()
		.create_with_conn(&mut db, &session)
		.await
		.unwrap();
	let credential = Authorization {
		pool: wb.f.store.pool.clone(),
	}
	.credentials("acme")
	.await
	.unwrap()
	.remove(0);
	let mapping = DashboardMapping::build()
		.id(Uuid::new_v4())
		.identity(identity.id)
		.tenant("acme")
		.subject("alice")
		.credential(credential.id)
		.enabled(true)
		.revision(1)
		.finish();
	// Mappings hold a row-level policy; seed this one in the fixture scope.
	wb.f.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			DashboardMapping::objects()
				.create_with_conn(tx, &mapping)
				.await?;
			Ok(())
		})
		.await
		.unwrap();
	let mut headers = HeaderMap::new();
	headers.insert(
		"cookie",
		"__Host-aidash-session=browser-authority-session"
			.parse()
			.unwrap(),
	);
	headers.insert("origin", "https://localhost:8080".parse().unwrap());
	headers.insert("x-aidash-csrf", "browser-authority-csrf".parse().unwrap());
	headers.insert(
		"x-aidash-context",
		format!("mapping:{}:{}", mapping.tenant, mapping.id)
			.parse()
			.unwrap(),
	);
	(wb, headers, session)
}

#[rstest::rstest]
#[case::google_status_age("https://accounts.google.com", "status_age", 200)]
#[case::generic_status_age("http://localhost/issuer", "status_age", 200)]
#[case::logout("https://accounts.google.com", "logout", 401)]
#[case::absolute_expiry("https://accounts.google.com", "expired", 401)]
#[case::idle_expiry("https://accounts.google.com", "idle", 401)]
#[case::disabled_identity("https://accounts.google.com", "disabled", 403)]
#[tokio::test]
async fn native_draft_mutations_recheck_browser_authority(
	#[case] _issuer: &str,
	#[case] change: &str,
	#[case] expected_status: u16,
	#[future(awt)]
	#[with(_issuer)]
	browser_workbench: (Workbench, HeaderMap, DashboardSession),
) {
	let (wb, headers, mut session) = browser_workbench;
	let (actor, _) =
		aidash_server::dashboard_auth::actor_from_headers(&wb.f, &headers, &Method::GET)
			.await
			.unwrap();
	// Act: capture the actor after applying this scenario authority configuration.
	let app = captured_actor_application(&wb.f, actor).await;
	let lease = reinhardt::db::orm::DatabaseConnectionLease::register(wb.f.store.pool.connection())
		.unwrap();
	let mut db = lease.handle();
	let now = Utc::now();
	match change {
		"logout" => {
			// Act: rebuild the browser context after the scenario authority configuration.
			let browser = common::application(wb.f.clone()).await;
			let client = browser.client();
			for (name, value) in &headers {
				client
					.set_header(name.as_str(), value.to_str().unwrap())
					.await
					.unwrap();
			}
			assert_eq!(
				client
					.post("/auth/logout", &Value::Null, "json")
					.await
					.unwrap()
					.status_code(),
				204
			);
		}
		"expired" | "idle" => {
			if change == "expired" {
				session.expires_at = now - Duration::seconds(1);
			} else {
				session.last_activity_at = now - Duration::hours(1);
			}
			DashboardSession::objects()
				.update_with_conn(&mut db, &session)
				.await
				.unwrap();
		}
		"status_age" | "disabled" => {
			let mut identity = DashboardIdentity::objects()
				.get(session.identity_id())
				.get_with_db(&mut db)
				.await
				.unwrap();
			if change == "disabled" {
				identity.disabled_at = Some(now);
			} else {
				identity.last_valid_at = Some(now - Duration::hours(1));
			}
			DashboardIdentity::objects()
				.update_with_conn(&mut db, &identity)
				.await
				.unwrap();
		}
		_ => panic!("unknown authority change: {change}"),
	}
	let (status, result) = request(&app, &wb.token, "PUT", &wb.path(), json!({"expected_revision":1,"entry":wb.draft["entry"],"documents":[],"release_notes":"Browser authority rechecked"})).await;
	assert_eq!(status, expected_status, "{change}: {result}");
	let (revision, release_notes): (i64, String) =
		sqlx::query_as("SELECT revision, release_notes FROM agent_drafts WHERE id = $1")
			.bind(uuid::Uuid::parse_str(wb.draft["id"].as_str().unwrap()).unwrap())
			.fetch_one(wb.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(revision, if expected_status == 200 { 2 } else { 1 });
	assert_eq!(
		release_notes,
		if expected_status == 200 {
			"Browser authority rechecked"
		} else {
			""
		}
	);
	wb.cleanup().await;
}

struct RemovedBinding;
impl aidash_application::ports::authorization::dashboard::Issuers for RemovedBinding {
	fn admitted(&self) -> Vec<String> {
		vec![]
	}
	fn restriction(&self, _: &str) -> Option<String> {
		None
	}
	fn restricted_issuer(&self, _: &str) -> Option<String> {
		None
	}
	fn retires_refused(&self) -> bool {
		true
	}
}

#[rstest::rstest]
#[case::limits("/api/workbench/test-limits/acme", "GET")]
#[case::profiles("/api/workbench/test-profiles/acme", "GET")]
#[case::profile_write("/api/workbench/test-profiles/acme/new", "PUT")]
#[tokio::test]
async fn a_removed_binding_disables_the_identity_before_workbench_authority_locks(
	#[case] path: &str,
	#[case] method: &str,
	#[future(awt)] browser_workbench: (Workbench, HeaderMap, DashboardSession),
) {
	use aidash_application::ports::authorization::dashboard::AccountPolicy;
	use aidash_server::apps::registry::workbench::views::{profile, test};
	let (mut wb, _, session) = browser_workbench;
	// A fresh self-issued bearer acts through the existing browser Mapping.
	let authorization = Authorization {
		pool: wb.f.store.pool.clone(),
	};
	let issued = authorization
		.issue_credential("acme", "alice", 3600, "fixture")
		.await
		.unwrap();
	wb.f.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			let mapping = DashboardMapping::objects()
				.filter(DashboardMapping::field_identity_id().eq(session.identity_id()))
				.get_with_db(tx)
				.await?;
			let mut credential =
				aidash_server::apps::identity::models::AuthorizationCredential::objects()
					.get(issued.credential.id)
					.get_with_db(tx)
					.await?;
			credential.mapping_id = Some(mapping.id);
			aidash_server::apps::identity::models::AuthorizationCredential::objects()
				.update_with_conn(tx, &credential)
				.await?;
			Ok(())
		})
		.await
		.unwrap();
	let actor = authorization.authenticate(&issued.token).await.unwrap();
	wb.f.store =
		wb.f.store
			.clone()
			.with_dashboard_policy(Some(AccountPolicy {
				issuers: std::sync::Arc::new(RemovedBinding),
				authority: None,
				personal_tenants: false,
				demo_data: false,
			}));
	let app = common::application_with(wb.f.clone(), |_| {
		reinhardt::ServerRouter::new()
			.endpoint(test::tenant_limits)
			.endpoint(profile::administered)
			.endpoint(profile::put)
			.with_middleware(super::CapturedActor(actor.clone()))
	})
	.await;
	let (status, response) = request(
		&app,
		&wb.token,
		method,
		path,
		json!({"expected_revision":0,"enabled":true,"rules":[]}),
	)
	.await;
	assert_eq!(status, 403, "{response}");
	let disabled =
		wb.f.store
			.pool
			.atomic(&TEST_FIXTURE, async |tx| {
				Ok(DashboardIdentity::objects()
					.get(session.identity_id())
					.get_with_db(tx)
					.await?
					.disabled_at)
			})
			.await
			.unwrap();
	assert!(
		disabled.is_some(),
		"refusal must durably disable the removed identity"
	);
	wb.cleanup().await;
}
