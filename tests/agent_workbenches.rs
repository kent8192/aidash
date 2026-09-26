mod common;
use common::{TestEnvironment, cleanup, request, setup, test_environment};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn finished_test(app: &axum::Router, token: &str, path: &str, id: &str) -> Value {
	for _ in 0..100 {
		let (_, sessions) = request(app, token, "GET", path, Value::Null).await;
		let session = sessions
			.as_array()
			.unwrap()
			.iter()
			.find(|item| item["id"] == id)
			.unwrap()
			.clone();
		if session["status"] != "running" {
			return session;
		}
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	panic!("test session did not finish: {id}");
}

fn agent() -> Value {
	json!({"id":"","version":"1.0.0","kind":"agent","name":{"en":"Test agent"},"description":{"en":"Authoring fixture"},"capabilities":["summarize"],"tags":[],"languages":["en"],"skills":[],"schema":{},"config":{"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Summarize carefully","tools":[],"skills":[],"cluster":null,"max_steps":8}})
}

#[rstest::rstest]
#[tokio::test]
async fn simulated_test_never_invokes_the_registered_external_tool(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let tool_hits = Arc::new(AtomicUsize::new(0));
	let hits = tool_hits.clone();
	let real_hits = Arc::new(AtomicUsize::new(0));
	let confined_hits = real_hits.clone();
	let mock = axum::Router::new()
		.route("/v1/chat/completions", axum::routing::post(|axum::Json(body): axum::Json<Value>| async move {
			let context = body["messages"][1]["content"].as_str().unwrap_or("");
			if context.contains("\"conversation\"") {
				axum::Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"Simulated result received"}}],"usage":{"prompt_tokens":40,"completion_tokens":6}}))
			} else {
			axum::Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[{"id":"call-1","function":{"name":"plugin_0","arguments":"{\"action\":\"read\",\"resource\":\"sandbox\",\"input\":\"safe\"}"}}]}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}))
			}
		}))
		.route("/effect", axum::routing::post(move || { let hits = hits.clone(); async move { hits.fetch_add(1, Ordering::SeqCst); axum::Json(json!({"effect":"unexpected"})) } }))
		.route("/test-effect", axum::routing::post(move || { let hits = confined_hits.clone(); async move { hits.fetch_add(1, Ordering::SeqCst); axum::Json(json!({"effect":"confined"})) } }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, mock).await.unwrap();
	});
	let (f, url, schema) = setup(&environment).await;
	let app = aidash::api::router(f.clone());
	let operator = f.config.api_token.clone();
	for (id, kind, config) in [
		(
			"fixture-model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"credential_env":null,"context_window":32768,"max_output_tokens":2048,"modalities":["text"],"cost":{}}),
		),
		(
			"fixture-tool",
			"tool",
			json!({"transport":"http","endpoint":format!("{endpoint}/effect"),"credential_env":null,"replay":"read_only"}),
		),
	] {
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"tags":[],"languages":["en"],"skills":[],"schema":{},"config":config});
		let (status, body) = request(&app, &operator, "POST", "/api/registry", entry).await;
		assert_eq!(status, 200, "register dependency: {body}");
	}
	let bundle = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[{"id":"creator","effect":"allow","subjects":{"ids":["alice"]},"actions":["agent_draft.create","agent_draft.read","agent_draft.write","agent_draft.test","agent_dependency.read"],"resources":{"kinds":["agent_draft","registry_entry"]}}]});
	assert_eq!(
		request(
			&app,
			&operator,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":0,"bundle":bundle})
		)
		.await
		.0,
		200
	);
	let (_, credential) = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let token = credential["token"].as_str().unwrap();
	let mut entry = agent();
	entry["config"]["tools"] = json!([{"id":"fixture-tool","version":"1.0.0"}]);
	let (status, draft) = request(
		&app,
		token,
		"POST",
		"/api/workbench/drafts",
		json!({"entry":entry}),
	)
	.await;
	assert_eq!(status, 200, "draft: {draft}");
	let path = format!(
		"/api/workbench/drafts/{}/tests",
		draft["id"].as_str().unwrap()
	);
	for (fixtures, expected) in [
		(json!({}), "blocked"),
		(
			json!({"plugin_0":{"status":"success","response":{"ok":true}}}),
			"completed",
		),
	] {
		let (status, session) = request(
			&app,
			token,
			"POST",
			&path,
			json!({"expected_revision":1,"message":"Please use the tool","fixtures":fixtures}),
		)
		.await;
		assert_eq!(status, 200, "start test: {session}");
		let id = session["id"].as_str().unwrap();
		let mut finished = Value::Null;
		for _ in 0..100 {
			let (_, sessions) = request(&app, token, "GET", &path, Value::Null).await;
			finished = sessions
				.as_array()
				.unwrap()
				.iter()
				.find(|item| item["id"] == id)
				.unwrap()
				.clone();
			if finished["status"] != "running" {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
		assert_eq!(finished["status"], expected, "session: {finished}");
		assert_eq!(finished["tool_calls"][0]["name"], "plugin_0");
	}
	assert_eq!(
		tool_hits.load(Ordering::SeqCst),
		0,
		"simulation reached a real tool endpoint"
	);
	let (status, profile) = request(&app, &operator, "PUT", "/api/workbench/test-profiles/acme/sandbox", json!({"expected_revision":0,"enabled":true,"rules":[{"tool":{"id":"fixture-tool","version":"1.0.0"},"endpoint":format!("{endpoint}/test-effect"),"credential_env":null,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]})).await;
	assert_eq!(status, 200, "profile: {profile}");
	let (status, visible) = request(
		&app,
		token,
		"GET",
		&format!(
			"/api/workbench/test-profiles?draft_id={}",
			draft["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "visible profiles: {visible}");
	assert_eq!(visible[0]["id"], "sandbox");
	assert!(
		visible[0].get("rules").is_none(),
		"profile scope should not leak from list"
	);
	let (status, real) = request(&app, token, "POST", &path, json!({"expected_revision":1,"message":"Use the confined Tool","mode":"real","profile_id":"sandbox"})).await;
	assert_eq!(status, 200, "real test: {real}");
	let id = real["id"].as_str().unwrap();
	let mut finished = Value::Null;
	for _ in 0..100 {
		let (_, sessions) = request(&app, token, "GET", &path, Value::Null).await;
		finished = sessions
			.as_array()
			.unwrap()
			.iter()
			.find(|item| item["id"] == id)
			.unwrap()
			.clone();
		if finished["status"] != "running" {
			break;
		}
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	assert_eq!(finished["status"], "completed", "real session: {finished}");
	assert_eq!(finished["tool_calls"][0]["outcome"], "real");
	assert_eq!(real_hits.load(Ordering::SeqCst), 1);
	assert_eq!(tool_hits.load(Ordering::SeqCst), 0);
	let (status, continued) = request(&app, token, "POST", &path, json!({"expected_revision":1,"message":"Continue the conversation","mode":"real","profile_id":"sandbox","continue_from":id})).await;
	assert_eq!(status, 200, "continue: {continued}");
	let continuation = finished_test(&app, token, &path, continued["id"].as_str().unwrap()).await;
	assert_eq!(
		continuation["status"], "completed",
		"continuation: {continuation}"
	);
	assert!(
		continuation["conversation"].as_array().unwrap().len()
			> finished["conversation"].as_array().unwrap().len()
	);
	assert_eq!(
		real_hits.load(Ordering::SeqCst),
		1,
		"continuation unexpectedly repeated a Tool"
	);
	let (status, reset) = request(&app, token, "POST", &path, json!({"expected_revision":1,"message":"Fresh conversation after reset","mode":"real","profile_id":"sandbox"})).await;
	assert_eq!(status, 200, "reset: {reset}");
	let reset = finished_test(&app, token, &path, reset["id"].as_str().unwrap()).await;
	assert_eq!(reset["status"], "completed", "reset session: {reset}");
	assert_eq!(real_hits.load(Ordering::SeqCst), 2);
	let (status, revised) = request(&app, &operator, "PUT", "/api/workbench/test-profiles/acme/sandbox", json!({"expected_revision":1,"enabled":true,"rules":[{"tool":{"id":"fixture-tool","version":"1.0.0"},"endpoint":format!("{endpoint}/test-effect"),"credential_env":null,"allowed_actions":["read"],"allowed_resources":["other"]}]})).await;
	assert_eq!(status, 200, "revised profile: {revised}");
	let (status, denied) = request(&app, token, "POST", &path, json!({"expected_revision":1,"message":"Try outside the test resource","mode":"real","profile_id":"sandbox"})).await;
	assert_eq!(status, 200, "denied test: {denied}");
	let denied = finished_test(&app, token, &path, denied["id"].as_str().unwrap()).await;
	assert_ne!(
		denied["status"], "completed",
		"out-of-profile call completed: {denied}"
	);
	assert_eq!(denied["status"], "blocked");
	assert_eq!(denied["tool_calls"][0]["outcome"], "denied");
	assert_eq!(
		real_hits.load(Ordering::SeqCst),
		2,
		"out-of-profile call reached the endpoint"
	);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn draft_conflict_register_and_factual_inspection(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = aidash::api::router(f.clone());
	let operator = f.config.api_token.clone();
	let model = json!({"id":"fixture-model","version":"1.0.0","kind":"model","name":{"en":"Fixture model"},"description":{"en":"Fixture"},"capabilities":[],"tags":[],"languages":["en"],"skills":[],"schema":{},"config":{"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9999/v1","credential_env":null,"context_window":32768,"max_output_tokens":2048,"modalities":["text"],"cost":{}}});
	let (status, body) = request(&app, &operator, "POST", "/api/registry", model).await;
	assert_eq!(status, 200, "model: {body}");
	let bundle = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[
		{"id":"creator","effect":"allow","subjects":{"ids":["alice"]},"actions":["agent_draft.create","agent_draft.read","agent_draft.write","agent_draft.register","agent_draft.test","agent_draft.share","agent_draft.transfer","agent_draft.archive","agent_dependency.read","agent_version.inspect","agent_incident.manage","agent_incident.read"],"resources":{"kinds":["agent_draft","registry_entry","agent_version","agent_incident"]}},
		{"id":"editor","effect":"allow","subjects":{"ids":["bob"]},"actions":["agent_draft.read","agent_draft.write"],"resources":{"kinds":["agent_draft"]}}
	]});
	let (status, body) = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":0,"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "policy: {body}");
	let (_, alice) = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let (_, bob) = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let alice_token = alice["token"].as_str().unwrap();
	let bob_token = bob["token"].as_str().unwrap();
	let (status, draft) = request(
		&app,
		alice_token,
		"POST",
		"/api/workbench/drafts",
		json!({"entry":agent(),"documents":[],"release_notes":"First version"}),
	)
	.await;
	assert_eq!(status, 200, "draft: {draft}");
	let draft_id = draft["id"].as_str().unwrap();
	let agent_id = draft["entry"]["id"].as_str().unwrap();
	assert_eq!(draft["entry"]["config"]["allow_task_creation"], false);
	let mut invalid = draft["entry"].clone();
	invalid["config"]["allow_task_creation"] = Value::Null;
	assert_eq!(
		request(
			&app,
			alice_token,
			"PUT",
			&format!("/api/workbench/drafts/{draft_id}"),
			json!({"expected_revision":1,"entry":invalid})
		)
		.await
		.0,
		400
	);
	assert_eq!(
		request(
			&app,
			bob_token,
			"GET",
			&format!("/api/workbench/drafts/{draft_id}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	let mut changed = draft["entry"].clone();
	changed["config"]["instructions"] = json!("Summarize accurately and cite sources");
	let path = format!("/api/workbench/drafts/{draft_id}");
	let (status, saved) = request(
		&app,
		alice_token,
		"PUT",
		&path,
		json!({"expected_revision":1,"entry":changed,"documents":[],"release_notes":"First version"}),
	)
	.await;
	assert_eq!(status, 200, "save: {saved}");
	assert_eq!(saved["revision"], 2);
	assert_eq!(
		request(
			&app,
			alice_token,
			"PUT",
			&path,
			json!({"expected_revision":1,"entry":changed,"documents":[],"release_notes":"stale"})
		)
		.await
		.0,
		409
	);
	let (status, validated) = request(
		&app,
		alice_token,
		"POST",
		&format!("{path}/validate"),
		json!({"expected_revision":2}),
	)
	.await;
	assert_eq!(status, 200, "validation: {validated}");
	assert_eq!(validated["valid"], true, "validation: {validated}");
	let register_path = format!("{path}/register");
	let (status, registration) = request(
		&app,
		alice_token,
		"POST",
		&register_path,
		json!({"expected_revision":2}),
	)
	.await;
	assert_eq!(status, 200, "register: {registration}");
	assert_eq!(registration["entry"]["id"], agent_id);
	assert_eq!(registration["behavioral_tested"], false);
	let (status, versions) = request(
		&app,
		alice_token,
		"GET",
		&format!("{path}/versions"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "versions: {versions}");
	assert_eq!(versions[0]["entry"]["id"], agent_id);
	assert_eq!(versions[0]["draft_revision"], 2);
	assert_eq!(versions[0]["release_notes"], "First version");
	assert_eq!(versions[0]["behavioral_tested"], false);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&register_path,
			json!({"expected_revision":2})
		)
		.await
		.0,
		200
	);
	let (_, catalog) = request(
		&app,
		&operator,
		"GET",
		"/api/authorization/acme/catalog",
		Value::Null,
	)
	.await;
	assert_eq!(catalog, json!([]));
	assert_eq!(
		request(
			&app,
			alice_token,
			"GET",
			&format!("/api/registry/{agent_id}/1.0.0"),
			Value::Null
		)
		.await
		.0,
		403
	);
	let trust_path = format!("/api/workbench/versions/{agent_id}/1.0.0");
	let (status, inspection) = request(&app, alice_token, "GET", &trust_path, Value::Null).await;
	assert_eq!(status, 200, "inspection: {inspection}");
	assert_eq!(inspection["external_assessment_available"], false);
	assert_eq!(inspection["workspaces"], json!([]));
	assert_eq!(inspection["test_evidence"], json!([]));
	let (status, incident) = request(&app, alice_token, "POST", &format!("{trust_path}/incidents"), json!({"severity":"medium","owner":"alice","notes":"Manual report","evidence":[{"title":"Observation","content":"Fixed copy"}]})).await;
	assert_eq!(status, 200, "incident: {incident}");
	let (status, archived_incident) = request(&app, alice_token, "PUT", &format!("/api/workbench/incidents/{}", incident["id"].as_str().unwrap()), json!({"expected_revision":1,"severity":"medium","status":"open","archived":true,"owner":"alice","notes":"Manual report","add_evidence":[]})).await;
	assert_eq!(status, 200, "archive open incident: {archived_incident}");
	assert_eq!(archived_incident["evidence_expires_at"], Value::Null);
	let (status, report) = request(
		&app,
		alice_token,
		"GET",
		&format!("{trust_path}/report?format=json"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "report: {report}");
	assert_eq!(report["permission_context"]["tenant"], "acme");
	assert_eq!(report["permission_context"]["subject"], "alice");
	assert!(
		report["note"]
			.as_str()
			.unwrap()
			.contains("no Trust assessment")
	);
	assert!(
		report["incidents"][0]["evidence"][0]
			.get("content")
			.is_none()
	);
	assert_eq!(
		aidash::workbench::purge_incident_evidence(&f.store.pool)
			.await
			.unwrap(),
		0
	);
	let incident_path = format!(
		"/api/workbench/incidents/{}",
		incident["id"].as_str().unwrap()
	);
	assert_eq!(request(&app, alice_token, "PUT", &incident_path, json!({"expected_revision":2,"severity":"medium","status":"resolved","archived":true,"owner":"alice","notes":"Manual report","add_evidence":[]})).await.0, 200);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("agent_incidents"))
			.value(
				sea_orm::sea_query::Alias::new("evidence_expires_at"),
				sea_orm::sea_query::Expr::cust("clock_timestamp() - interval '1 day'"),
			)
			.and_where(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id"))
					.eq(sea_orm::sea_query::Expr::cust("$1")),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(
		incident["id"]
			.as_str()
			.unwrap()
			.parse::<uuid::Uuid>()
			.unwrap(),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		aidash::workbench::purge_incident_evidence(&f.store.pool)
			.await
			.unwrap(),
		1
	);
	let (_, expired) = request(&app, alice_token, "GET", &incident_path, Value::Null).await;
	assert_eq!(expired["evidence"][0]["content"], Value::Null);
	assert!(expired["evidence_expired_at"].is_string());
	let (status, reopened) = request(&app, alice_token, "PUT", &incident_path, json!({"expected_revision":3,"severity":"medium","status":"open","archived":false,"owner":"alice","notes":"Reopened after expiry","add_evidence":[]})).await;
	assert_eq!(status, 200, "reopen expired incident: {reopened}");
	assert_eq!(reopened["evidence"][0]["content"], Value::Null);
	let share_path = format!("{path}/shares");
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&share_path,
			json!({"subject":"bob","can_edit":false,"enabled":true,"include_documents":false})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, bob_token, "GET", &path, Value::Null).await.0,
		200
	);
	assert_eq!(
		request(
			&app,
			bob_token,
			"PUT",
			&path,
			json!({"expected_revision":2,"entry":saved["entry"]})
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&share_path,
			json!({"subject":"bob","can_edit":true,"enabled":true,"include_documents":false})
		)
		.await
		.0,
		200
	);
	let (status, edited) = request(
		&app,
		bob_token,
		"PUT",
		&path,
		json!({"expected_revision":2,"entry":saved["entry"],"release_notes":"Editor update"}),
	)
	.await;
	assert_eq!(status, 200, "shared edit: {edited}");
	assert_eq!(edited["revision"], 3);
	assert_eq!(
		request(
			&app,
			bob_token,
			"POST",
			&register_path,
			json!({"expected_revision":3})
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&register_path,
			json!({"expected_revision":3})
		)
		.await
		.0,
		409
	);
	let private_documents =
		json!([{"name":"notes.txt","media_type":"text/plain","text":"Private note"}]);
	let (status, with_documents) = request(&app, alice_token, "PUT", &path, json!({"expected_revision":3,"entry":edited["entry"],"documents":private_documents,"release_notes":"Add reference"})).await;
	assert_eq!(status, 200, "add private document: {with_documents}");
	assert_eq!(
		request(&app, bob_token, "GET", &path, Value::Null).await.0,
		403
	);
	let (_, shares) = request(&app, alice_token, "GET", &share_path, Value::Null).await;
	assert_eq!(shares[0]["documents_current"], false);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&share_path,
			json!({"subject":"bob","can_edit":true,"enabled":true,"include_documents":false})
		)
		.await
		.0,
		400
	);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&share_path,
			json!({"subject":"bob","can_edit":true,"enabled":true,"include_documents":true})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, bob_token, "GET", &path, Value::Null).await.0,
		200
	);
	let (status, duplicate) = request(
		&app,
		alice_token,
		"POST",
		&format!("{path}/duplicate"),
		json!({"expected_revision":4}),
	)
	.await;
	assert_eq!(status, 200, "duplicate: {duplicate}");
	assert_ne!(duplicate["entry"]["id"], agent_id);
	assert_eq!(duplicate["source_id"], agent_id);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&format!("{path}/archive"),
			json!({"expected_revision":4,"archived":true})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&format!("{path}/archive"),
			json!({"expected_revision":5,"archived":false})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			alice_token,
			"POST",
			&format!("{path}/transfer"),
			json!({"expected_revision":6,"new_owner":"bob"})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, alice_token, "GET", &path, Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		request(&app, bob_token, "GET", &path, Value::Null).await.0,
		200
	);
	let restricted = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[
		{"id":"creator","effect":"allow","subjects":{"ids":["alice"]},"actions":["agent_draft.create","agent_draft.read","agent_draft.write","agent_draft.register","agent_draft.test","agent_dependency.read"],"resources":{"kinds":["agent_draft","registry_entry"]}}
	]});
	assert_eq!(
		request(
			&app,
			&operator,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":restricted})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, alice_token, "GET", &trust_path, Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		request(
			&app,
			alice_token,
			"GET",
			&format!("{trust_path}/report?format=json"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(&app, alice_token, "GET", &path, Value::Null)
			.await
			.0,
		403
	);
	cleanup(f, &url, &schema).await;
}
