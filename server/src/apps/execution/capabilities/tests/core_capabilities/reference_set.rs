//! A Reference set: who can read it, how it is added and how an Agent finds it.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

pub(super) fn file(path: &str, text: &str) -> Value {
	json!({"path":path,"data":STANDARD.encode(text)})
}

pub(super) async fn add(c: &CoreFixture, token: &str, files: Vec<Value>) -> (u16, Value) {
	request(
		&c.app,
		token,
		"POST",
		"/api/references/batch",
		json!({"files":files}),
	)
	.await
}

pub(super) fn agent_subject(c: &CoreFixture, version: &str) -> String {
	aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(c.f.config.deployment.as_str(), "acme"),
		"research",
		version,
	)
}

async fn register_source(
	c: &CoreFixture,
	token: &str,
	id: &str,
	references: &Value,
) -> (u16, Value) {
	request(
		&c.app,
		token,
		"POST",
		"/api/registry",
		json!({"id":id,"version":"1.0.0","kind":"source","name":{"en":id},"description":{"en":"Project documents"},"schema":{},"config":{"schema_version":1,"source":{"adapter":"reference_attachments","references":references}}}),
	)
	.await
}

/// Registers and approves `research@1.2.0` bound to these Sources. Its subject
/// holds no reference action, as an Agent on the default policy does not.
pub(super) async fn bind_agent(c: &mut CoreFixture, sources: &[(&str, Value)]) {
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let mut bindings = vec![];
	for (id, references) in sources {
		let (status, source) = register_source(c, &c.token, id, references).await;
		assert_eq!(status, 200, "{source}");
		let (status, approved) = request(
			&c.app,
			&c.token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"node":node,"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{approved}");
		bindings.push(json!({"kind":"source","target":{"registry_deployment":c.f.config.deployment,"id":id,"version":"1.0.0"},"narrow":{}}));
	}
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut agent =
		c.f.registry
			.get(&tenant, "research", "1.1.0")
			.await
			.unwrap();
	agent.version = "1.2.0".into();
	agent.binding_normalization = None;
	agent.config["bindings"]
		.as_array_mut()
		.unwrap()
		.extend(bindings);
	// Each tool call of a scripted Run spends about ten worker steps.
	agent.config["max_steps"] = json!(200);
	let (status, saved) = request(&c.app, &c.token, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{saved}");
	let subject = agent_subject(c, "1.2.0");
	c.policy["subjects"][&subject] = json!({"kind":"agent","node":node});
	c.policy["policies"]
		.as_array_mut()
		.unwrap()
		.push(json!({"id":"agent-reads-no-reference","effect":"deny","subjects":{"ids":[subject]},"actions":["reference.read","reference.upload","reference.delete"],"resources":{"kinds":["reference"]}}));
	let (status, updated) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{updated}");
	let (status, approved) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":node,"entry":{"id":"research","version":"1.2.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
}

async fn start(c: &CoreFixture, token: &str, task: Uuid) -> (u16, Value) {
	request(
		&c.app,
		token,
		"POST",
		&format!("/api/tasks/{task}/delegate"),
		json!({"node_id":c.f.config.deployment,"agent":{"id":"research","version":"1.2.0"}}),
	)
	.await
}

async fn latest_run(c: &CoreFixture) -> aidash_server::domain::Run {
	c.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0)
}

async fn paths(c: &CoreFixture, token: &str, run: Uuid) -> Vec<String> {
	let (status, found) = request(
		&c.app,
		token,
		"POST",
		&format!("/api/runs/{run}/files/search"),
		json!({"query":"","mode":"path","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	found["matches"]
		.as_array()
		.unwrap()
		.iter()
		.map(|item| item["path"].as_str().unwrap().to_owned())
		.collect()
}

#[rstest::rstest]
#[tokio::test]
async fn an_agent_with_no_reference_action_reads_the_files_its_version_binds(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (status, added) = add(
		&c,
		&token,
		vec![
			file("backend/api.md", "# 注文 API\n注文を作成する\n"),
			file("frontend/api.md", "# 画面\n注文画面\n"),
		],
	)
	.await;
	assert_eq!(status, 200, "{added}");
	bind_agent(&mut c, &[("project-docs", added["references"].clone())]).await;

	let (status, started) = start(&c, &token, c.task).await;

	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	let listed = paths(&c, &token, run.id).await;
	assert_eq!(listed.len(), 2, "{listed:?}");
	assert!(listed.contains(&"references/backend/api.md".to_owned()));
	assert!(listed.contains(&"references/frontend/api.md".to_owned()));
	let (status, found) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"注文を作成","mode":"literal","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	assert_eq!(found["matches"].as_array().unwrap().len(), 1, "{found}");
	assert_eq!(found["matches"][0]["path"], "references/backend/api.md");
	let (status, read) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":found["matches"][0]["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert!(read["content"].as_str().unwrap().contains("注文を作成する"));
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn another_person_can_run_the_agent_but_cannot_download_its_files(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let alice = c.token.clone();
	let (status, added) = add(&c, &alice, vec![file("private/plan.md", "alice の計画\n")]).await;
	assert_eq!(status, 200, "{added}");
	let id = added["references"][0]["reference_id"]
		.as_str()
		.unwrap()
		.to_owned();
	bind_agent(&mut c, &[("alice-docs", added["references"].clone())]).await;
	let credential = common::issued_credential(&c.f.store, "acme", "bob").await;
	let bob = credential["token"].as_str().unwrap().to_owned();
	let run = {
		let (status, _) = start(&c, &alice, c.task).await;
		assert_eq!(status, 200);
		latest_run(&c).await
	};
	let (status, task) = request(
		&c.app,
		&bob,
		"POST",
		&format!("/api/workspaces/{}/tasks", run.workspace_id),
		json!({"title":"Bob's work","description":"Read the plan"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let task: Uuid = serde_json::from_value(task["id"].clone()).unwrap();

	let (status, started) = start(&c, &bob, task).await;

	assert_eq!(status, 200, "{started}");
	let run =
		c.f.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id == task)
			.unwrap();
	assert_eq!(
		paths(&c, &bob, run.id).await,
		vec!["references/private/plan.md"]
	);
	let (status, denied) = request(
		&c.app,
		&bob,
		"GET",
		&format!("/api/references/{id}/download"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 404, "{denied}");
	let (status, area) = request(
		&c.app,
		&bob,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let (status, found) = request(
		&c.app,
		&bob,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"","mode":"path","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	let (status, denied) = request(
		&c.app,
		&bob,
		"GET",
		&format!(
			"/api/working-areas/{}/files/{}/download",
			area["id"].as_str().unwrap(),
			found["matches"][0]["file_id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 404, "{denied}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_source_cannot_bind_a_file_its_registrant_cannot_read(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let (status, added) = add(&c, &c.token, vec![file("secret.md", "alice only\n")]).await;
	assert_eq!(status, 200, "{added}");
	let credential = common::issued_credential(&c.f.store, "acme", "bob").await;
	let bob = credential["token"].as_str().unwrap().to_owned();

	let (status, refused) = register_source(&c, &bob, "stolen-docs", &added["references"]).await;

	assert_eq!(status, 404, "{refused}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn adding_documents_twice_creates_no_duplicates_and_reports_each_file(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let files = vec![
		file("a/one.md", "one\n"),
		file("b/one.md", "two\n"),
		file("c/copy.md", "one\n"),
		file("../escape.md", "x\n"),
		json!({"path":"empty.md","data":""}),
		file("a/one.md", "again\n"),
		file(&format!("long/{}.md", "長".repeat(90)), "long name\n"),
		file("a/one.md.extracted.txt", "reserved for an extraction\n"),
	];

	let (status, first) = add(&c, &c.token, files.clone()).await;
	assert_eq!(status, 200, "{first}");
	let (status, second) = add(&c, &c.token, files).await;
	assert_eq!(status, 200, "{second}");

	assert_eq!(first["references"], second["references"]);
	assert_eq!(first["references"].as_array().unwrap().len(), 2, "{first}");
	let outcome = &first["files"];
	assert_eq!(outcome[0]["reference"]["state"], "ready", "{first}");
	assert_eq!(outcome[0]["reference"]["extraction"], Value::Null);
	assert_eq!(outcome[0]["reused"], false);
	assert_eq!(second["files"][0]["reused"], true);
	assert_eq!(outcome[2]["duplicate_of"], "a/one.md");
	assert_eq!(outcome[3]["error"], "INVALID_PATH");
	assert_eq!(outcome[4]["error"], "REFERENCE_UPLOAD_LIMIT");
	assert_eq!(outcome[5]["error"], "DUPLICATE_PATH");
	assert_eq!(outcome[6]["error"], "REFERENCE_UPLOAD_LIMIT");
	assert_eq!(outcome[7]["error"], "INVALID_PATH");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn text_that_is_not_utf8_keeps_its_original_and_says_why_it_cannot_be_read(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let shift_jis = STANDARD.encode([0x93_u8, 0xfa, 0x96, 0x7b, 0x8c, 0xea]);

	let (status, added) = add(
		&c,
		&c.token,
		vec![json!({"path":"legacy.txt","data":shift_jis})],
	)
	.await;

	assert_eq!(status, 200, "{added}");
	assert_eq!(added["files"][0]["reference"]["state"], "ready", "{added}");
	assert_eq!(
		added["files"][0]["reference"]["extraction_state"],
		"unsupported"
	);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_same_path_in_two_sources_is_refused_at_registration(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (_, first) = add(&c, &token, vec![file("README.md", "first\n")]).await;
	let (_, second) = add(&c, &token, vec![file("README.md", "second\n")]).await;
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let mut bindings = vec![];
	for (id, added) in [("first-docs", &first), ("second-docs", &second)] {
		let (status, source) = register_source(&c, &token, id, &added["references"]).await;
		assert_eq!(status, 200, "{source}");
		let (status, approved) = request(
			&c.app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"node":node,"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{approved}");
		bindings.push(json!({"kind":"source","target":{"registry_deployment":c.f.config.deployment,"id":id,"version":"1.0.0"},"narrow":{}}));
	}
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut agent =
		c.f.registry
			.get(&tenant, "research", "1.1.0")
			.await
			.unwrap();
	agent.version = "1.2.0".into();
	agent.binding_normalization = None;
	agent.config["bindings"]
		.as_array_mut()
		.unwrap()
		.extend(bindings);

	let (status, refused) = request(&c.app, &token, "POST", "/api/registry", json!(agent)).await;

	assert_eq!(status, 400, "{refused}");
	let (status, repeated) = register_source(
		&c,
		&token,
		"repeated-docs",
		&json!([
			first["references"][0],
			{"reference_id":second["references"][0]["reference_id"],"digest":second["references"][0]["digest"],"path":"README.md"}
		]),
	)
	.await;
	assert_eq!(status, 400, "{repeated}");
	c.policy["policies"] = c.policy["policies"].clone();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn revoking_one_file_refuses_new_runs_and_names_its_path(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (status, added) = add(
		&c,
		&token,
		vec![file("keep.md", "keep\n"), file("gone/plan.md", "gone\n")],
	)
	.await;
	assert_eq!(status, 200, "{added}");
	bind_agent(&mut c, &[("revocable-docs", added["references"].clone())]).await;
	let revoked = &added["files"][1]["reference"];
	let (status, result) = request(
		&c.app,
		&token,
		"POST",
		&format!(
			"/api/references/{}/revoke",
			revoked["reference_id"].as_str().unwrap()
		),
		json!({"expected_revision":revoked["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{result}");

	let (status, refused) = start(&c, &token, c.task).await;

	assert_eq!(status, 404, "{refused}");
	assert!(refused.to_string().contains("gone/plan.md"), "{refused}");
	c.close().await;
}

/// The default test policy lets everyone do everything. This one leaves Alice
/// the uploader's whole authority and gives Bob, if anyone, only `reference.delete`.
fn only_alice_uses_references(c: &mut CoreFixture, bob_may_revoke: bool) {
	let resources = c.policy["policies"][0]["resources"].clone();
	c.policy["policies"][0]["subjects"] = json!({"kinds":["agent"]});
	let policies = c.policy["policies"].as_array_mut().unwrap();
	policies.push(json!({"id":"alice-works","effect":"allow","subjects":{"ids":["alice"]},"actions":["*"],"resources":resources}));
	if bob_may_revoke {
		policies.push(json!({"id":"bob-revokes-references","effect":"allow","subjects":{"ids":["bob"]},"actions":["reference.delete"],"resources":{"kinds":["reference"]}}));
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_subject_the_policy_grants_reference_delete_revokes_a_file_it_did_not_upload(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (status, added) = add(&c, &token, vec![file("gone/plan.md", "gone\n")]).await;
	assert_eq!(status, 200, "{added}");
	only_alice_uses_references(&mut c, true);
	bind_agent(
		&mut c,
		&[("bobs-revocable-docs", added["references"].clone())],
	)
	.await;
	let id = added["files"][0]["reference"]["reference_id"]
		.as_str()
		.unwrap()
		.to_owned();
	let bob = common::issued_credential(&c.f.store, "acme", "bob").await;
	let bob = bob["token"].as_str().unwrap();

	// Reading and downloading stay with the uploader.
	let (status, read) = request(
		&c.app,
		bob,
		"GET",
		&format!("/api/references/{id}"),
		json!(null),
	)
	.await;
	assert_eq!(status, 404, "{read}");
	let (status, download) = request(
		&c.app,
		bob,
		"GET",
		&format!("/api/references/{id}/download"),
		json!(null),
	)
	.await;
	assert_eq!(status, 404, "{download}");
	// The revocation view gives the revision and state, and no content.
	let (status, view) = request(
		&c.app,
		bob,
		"GET",
		&format!("/api/references/{id}/revocation"),
		json!(null),
	)
	.await;
	assert_eq!(status, 200, "{view}");
	assert_eq!(view["state"], "ready", "{view}");
	assert_eq!(view.as_object().unwrap().len(), 3, "{view}");
	let (status, stale) = request(
		&c.app,
		bob,
		"POST",
		&format!("/api/references/{id}/revoke"),
		json!({"expected_revision":view["revision"].as_i64().unwrap() + 1}),
	)
	.await;
	assert_eq!(status, 409, "{stale}");
	let (status, result) = request(
		&c.app,
		bob,
		"POST",
		&format!("/api/references/{id}/revoke"),
		json!({"expected_revision":view["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{result}");

	let (status, refused) = start(&c, &token, c.task).await;
	assert_eq!(status, 404, "{refused}");
	assert!(refused.to_string().contains("gone/plan.md"), "{refused}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_subject_the_policy_does_not_grant_reference_delete_gets_404_and_revokes_nothing(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (status, added) = add(&c, &token, vec![file("keep.md", "keep\n")]).await;
	assert_eq!(status, 200, "{added}");
	only_alice_uses_references(&mut c, false);
	bind_agent(&mut c, &[("kept-docs", added["references"].clone())]).await;
	let reference = &added["files"][0]["reference"];
	let id = reference["reference_id"].as_str().unwrap();
	let bob = common::issued_credential(&c.f.store, "acme", "bob").await;
	let bob = bob["token"].as_str().unwrap();

	let (status, view) = request(
		&c.app,
		bob,
		"GET",
		&format!("/api/references/{id}/revocation"),
		json!(null),
	)
	.await;
	assert_eq!(status, 404, "{view}");
	let (status, result) = request(
		&c.app,
		bob,
		"POST",
		&format!("/api/references/{id}/revoke"),
		json!({"expected_revision":reference["revision"]}),
	)
	.await;
	assert_eq!(status, 404, "{result}");

	let (status, started) = start(&c, &token, c.task).await;
	assert_eq!(status, 200, "{started}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn revoking_a_file_does_not_lock_an_area_that_never_copied_it(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (_, added) = add(&c, &token, vec![file("plan.md", "plan\n")]).await;
	bind_agent(&mut c, &[("short-lived-docs", added["references"].clone())]).await;
	let (status, started) = start(&c, &token, c.task).await;
	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	let reference = &added["files"][0]["reference"];
	let (status, result) = request(
		&c.app,
		&token,
		"POST",
		&format!(
			"/api/references/{}/revoke",
			reference["reference_id"].as_str().unwrap()
		),
		json!({"expected_revision":reference["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{result}");

	let (status, area) = request(
		&c.app,
		&token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;

	assert_eq!(status, 200, "{area}");
	c.close().await;
}

/// A revoked file is gone from the Area's file routes too, even for a Run that has ended
/// or is still the Area's latest, while the Area and its other files stay usable.
#[rstest::rstest]
#[tokio::test]
async fn a_revoked_file_stops_being_readable_through_the_run_file_routes(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (status, added) = add(
		&c,
		&token,
		vec![
			file("keep.md", "needle in the kept file\n"),
			file("gone.md", "needle in the revoked file\n"),
		],
	)
	.await;
	assert_eq!(status, 200, "{added}");
	bind_agent(&mut c, &[("secret-docs", added["references"].clone())]).await;
	let (status, started) = start(&c, &token, c.task).await;
	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	let (status, found) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"needle","mode":"literal","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	let id_of = |path: &str| {
		found["matches"]
			.as_array()
			.unwrap()
			.iter()
			.find(|item| item["path"] == path)
			.unwrap_or_else(|| panic!("{path} not found in {found}"))
			.clone()
	};
	let (kept, gone) = (id_of("references/keep.md"), id_of("references/gone.md"));
	let revoked = &added["files"][1]["reference"];
	let (status, result) = request(
		&c.app,
		&token,
		"POST",
		&format!(
			"/api/references/{}/revoke",
			revoked["reference_id"].as_str().unwrap()
		),
		json!({"expected_revision":revoked["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{result}");

	let (status, denied) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":gone["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 404, "{denied}");
	assert!(!denied.to_string().contains("revoked file"), "{denied}");
	let (status, found) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"needle","mode":"literal","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	assert_eq!(found["matches"].as_array().unwrap().len(), 1, "{found}");
	assert_eq!(found["matches"][0]["path"], "references/keep.md");
	assert_eq!(
		paths(&c, &token, run.id).await,
		vec!["references/keep.md"],
		"the listing omits the revoked file"
	);
	let (status, read) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":kept["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	let (status, area) = request(
		&c.app,
		&token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let (status, denied) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/materialize", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"path":"copy.md","source":{"kind":"file","file_id":gone["file_id"],"expected_digest":gone["digest"]}}),
	)
	.await;
	assert_eq!(status, 404, "{denied}");
	c.close().await;
}

/// A Tenant Administrator stops a Source's files by withdrawing its Catalog
/// Approval. The Run's file routes then answer as its worker does (#788).
#[rstest::rstest]
#[tokio::test]
async fn withdrawing_a_sources_approval_stops_the_run_file_routes(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (status, added) = add(&c, &token, vec![file("plan.md", "needle\n")]).await;
	assert_eq!(status, 200, "{added}");
	bind_agent(&mut c, &[("withdrawn-docs", added["references"].clone())]).await;
	let (status, started) = start(&c, &token, c.task).await;
	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	let (status, found) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"","mode":"path","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	let plan = found["matches"][0].clone();
	let (status, area) = request(
		&c.app,
		&token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let approve = |revision: i64, enabled: bool| {
		request(
			&c.app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"withdrawn-docs","version":"1.0.0"},"expected_revision":revision,"enabled":enabled}),
		)
	};
	let (status, withdrawn) = approve(1, false).await;
	assert_eq!(status, 200, "{withdrawn}");

	for (route, input) in [
		(
			"files/read",
			json!({"file_id":plan["file_id"],"representation":"text"}),
		),
		(
			"files/search",
			json!({"query":"needle","mode":"literal","scope":"references"}),
		),
		(
			"files/materialize",
			json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"path":"copy.md","source":{"kind":"file","file_id":plan["file_id"],"expected_digest":plan["digest"]}}),
		),
	] {
		let (status, denied) = request(
			&c.app,
			&token,
			"POST",
			&format!("/api/runs/{}/{route}", run.id),
			input,
		)
		.await;
		assert_eq!(status, 403, "{route}: {denied}");
		assert!(!denied.to_string().contains("needle"), "{route}: {denied}");
	}

	// Approving the Source again gives the files back.
	let (status, approved) = approve(withdrawn["revision"].as_i64().unwrap(), true).await;
	assert_eq!(status, 200, "{approved}");
	let (status, read) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":plan["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "needle\n", "{read}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_copy_of_a_reference_file_downloads_only_for_its_uploader(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let alice = c.token.clone();
	let (_, added) = add(&c, &alice, vec![file("plan.md", "alice plan\n")]).await;
	bind_agent(&mut c, &[("copied-docs", added["references"].clone())]).await;
	let credential = common::issued_credential(&c.f.store, "acme", "bob").await;
	let bob = credential["token"].as_str().unwrap().to_owned();
	let (status, _) = start(&c, &alice, c.task).await;
	assert_eq!(status, 200);
	let workspace = latest_run(&c).await.workspace_id;
	let (status, task) = request(
		&c.app,
		&bob,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Bob's work","description":"Copy the plan"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let task: Uuid = serde_json::from_value(task["id"].clone()).unwrap();
	let (status, started) = start(&c, &bob, task).await;
	assert_eq!(status, 200, "{started}");
	let run =
		c.f.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id == task)
			.unwrap();
	let (status, area) = request(
		&c.app,
		&bob,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let (status, found) = request(
		&c.app,
		&bob,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"","mode":"path","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");

	let (status, copy) = request(
		&c.app,
		&bob,
		"POST",
		&format!("/api/runs/{}/files/materialize", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"path":"copy.md","source":{"kind":"file","file_id":found["matches"][0]["file_id"],"expected_digest":found["matches"][0]["digest"]}}),
	)
	.await;

	assert_eq!(status, 200, "{copy}");
	let (status, denied) = request(
		&c.app,
		&bob,
		"GET",
		&format!(
			"/api/working-areas/{}/files/{}/download",
			area["id"].as_str().unwrap(),
			copy["file"]["file_id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 404, "{denied}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_agent_can_write_the_same_path_as_a_reference_file_without_losing_either(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let (_, added) = add(&c, &token, vec![file("plan.md", "the document\n")]).await;
	bind_agent(&mut c, &[("patched-docs", added["references"].clone())]).await;
	let (status, started) = start(&c, &token, c.task).await;
	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	let (_, area) = request(
		&c.app,
		&token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;

	let (status, patched) = request(
		&c.app,
		&token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"references/plan.md":null},"patch":"*** Begin Patch\n*** Add File: references/plan.md\n+my notes\n*** End Patch"}),
	)
	.await;

	assert_eq!(status, 200, "{patched}");
	assert_eq!(
		paths(&c, &token, run.id).await,
		vec!["references/plan.md"],
		"the Reference file stays"
	);
	let (status, written) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"my notes","mode":"literal","scope":"working"}),
	)
	.await;
	assert_eq!(status, 200, "{written}");
	assert_eq!(written["matches"].as_array().unwrap().len(), 1, "{written}");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn another_person_can_build_an_agent_version_on_an_approved_source(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let alice = c.token.clone();
	let (_, added) = add(&c, &alice, vec![file("plan.md", "alice plan\n")]).await;
	bind_agent(&mut c, &[("shared-docs", added["references"].clone())]).await;
	let credential = common::issued_credential(&c.f.store, "acme", "bob").await;
	let bob = credential["token"].as_str().unwrap().to_owned();

	let (status, saved) = request(
		&c.app,
		&bob,
		"POST",
		"/api/agents/research/capabilities",
		json!({"idempotency_key":Uuid::new_v4(),"source_version":"1.2.0","new_version":"1.3.0","bindings":[{"kind":"source","target":{"registry_deployment":c.f.config.deployment,"id":"shared-docs","version":"1.0.0"},"narrow":{}}],"remove_default":[]}),
	)
	.await;

	assert_eq!(status, 200, "{saved}");
	c.close().await;
}

/// A whole project's documents, timed against an Agent with none (#719).
#[rstest::rstest]
#[tokio::test]
async fn ninety_documents_register_and_a_run_finds_a_passage_within_budget(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let line = "注文の取り消し条件と在庫の引き当てについて説明する。\n";
	let body = line.repeat(2_000_000 / 90 / line.len());
	let files = (0..90)
		.map(|index| {
			file(
				&format!("section-{}/doc-{index}.md", index % 9),
				&format!("marker-{index}\n{body}"),
			)
		})
		.collect::<Vec<_>>();

	let added_at = std::time::Instant::now();
	let (status, added) = add(&c, &token, files).await;
	let adding = added_at.elapsed();

	assert_eq!(status, 200);
	assert_eq!(added["references"].as_array().unwrap().len(), 90);
	bind_agent(&mut c, &[("whole-project", added["references"].clone())]).await;
	let started_at = std::time::Instant::now();
	let (status, started) = start(&c, &token, c.task).await;
	let starting = started_at.elapsed();
	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	assert_eq!(
		paths(&c, &token, run.id).await.len(),
		50,
		"one listing page"
	);
	let searched_at = std::time::Instant::now();
	let (status, found) = request(
		&c.app,
		&token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"marker-89","mode":"literal","scope":"references"}),
	)
	.await;
	let searching = searched_at.elapsed();
	assert_eq!(status, 200, "{found}");
	assert_eq!(
		found["matches"][0]["path"], "references/section-8/doc-89.md",
		"{found}"
	);
	println!("#719 timing: add {adding:?}, run start {starting:?}, search {searching:?}");
	assert!(
		searching < std::time::Duration::from_secs(5),
		"{searching:?}"
	);
	c.close().await;
}

/// Every published revision holds one row per file of its manifest, Reference
/// files included (#789). A patch takes the same publication path as a shell
/// command, without needing a Runner.
#[rstest::rstest]
#[case::one_document(1)]
#[case::a_full_set(aidash_domain::capabilities::references::MAX_REFERENCE_FILES)]
#[tokio::test]
async fn every_published_revision_holds_one_row_per_file_of_its_manifest(
	#[future] capability_fixture: CoreFixture,
	#[case] documents: usize,
) {
	use reinhardt::query::{Alias, Expr, Func, PostgresQueryBuilder, Query};
	const PATCHES: usize = 5;
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let files = (0..documents)
		.map(|index| file(&format!("doc-{index}.md"), &format!("document {index}\n")))
		.collect::<Vec<_>>();
	let (status, added) = add(&c, &token, files).await;
	assert_eq!(status, 200, "{added}");
	bind_agent(&mut c, &[("sized-set", added["references"].clone())]).await;
	let (status, started) = start(&c, &token, c.task).await;
	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	let (_, area) = request(
		&c.app,
		&token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let first_revision = area["revision"].as_i64().unwrap();

	let mut revision = area["revision"].clone();
	for index in 0..PATCHES {
		let (status, patched) = request(
			&c.app,
			&token,
			"POST",
			&path_for_patch(run.id),
			json!({"idempotency_key":Uuid::new_v4(),"expected_revision":revision,"preconditions":{format!("note-{index}.md"):null},"patch":format!("*** Begin Patch\n*** Add File: note-{index}.md\n+note {index}\n*** End Patch")}),
		)
		.await;
		assert_eq!(status, 200, "{patched}");
		revision = patched["revision"].clone();
	}

	let rows = |revision: i64| {
		Query::select()
			.expr(Func::count(Expr::col(Alias::new("id")).into()))
			.from(Alias::new("core_blob_references"))
			.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(area_id)))
			.and_where(Expr::col(Alias::new("revision")).eq(Expr::value(revision)))
			.and_where(Expr::col(Alias::new("holder_kind")).eq(Expr::value("revision")))
			.to_string(PostgresQueryBuilder)
	};
	let pool = c.f.store.pool.fixture_driver();
	for (written, revision) in (first_revision + 1..).take(PATCHES).enumerate() {
		let count: i64 = sqlx::query_scalar(&rows(revision))
			.fetch_one(pool)
			.await
			.unwrap();
		assert_eq!(
			count as usize,
			documents + written + 1,
			"revision {revision} holds a row for each Reference file and each note"
		);
	}
	c.close().await;
}

/// Publishing must write each new revision reference once, in one statement.
#[rstest::rstest]
#[tokio::test]
async fn publication_batches_new_revision_references_without_rewriting_the_old_revision(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let token = c.token.clone();
	let files = (0..4)
		.map(|i| file(&format!("doc-{i}.md"), &format!("document {i}\n")))
		.collect();
	let (status, added) = add(&c, &token, files).await;
	assert_eq!(status, 200, "{added}");
	bind_agent(&mut c, &[("batch-count", added["references"].clone())]).await;
	let (status, started) = start(&c, &token, c.task).await;
	assert_eq!(status, 200, "{started}");
	let run = latest_run(&c).await;
	let (status, area) = request(
		&c.app,
		&token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");

	// Fixture-only triggers count attempted revision rows, including conflicts,
	// and statements that inserted revision rows. Query cannot express trigger DDL.
	let owner = common::owner_pool(c.f.store.pool.fixture_driver()).await;
	for ddl in [
		"CREATE TABLE revision_write_probe (kind text NOT NULL)",
		"GRANT INSERT, SELECT ON revision_write_probe TO PUBLIC",
		"CREATE FUNCTION count_revision_row() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.holder_kind = 'revision' THEN INSERT INTO revision_write_probe VALUES ('row'); END IF; RETURN NEW; END $$",
		"CREATE TRIGGER count_revision_row BEFORE INSERT ON core_blob_references FOR EACH ROW EXECUTE FUNCTION count_revision_row()",
		"CREATE FUNCTION count_revision_statement() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF EXISTS (SELECT 1 FROM inserted_refs WHERE holder_kind = 'revision') THEN INSERT INTO revision_write_probe VALUES ('statement'); END IF; RETURN NULL; END $$",
		"CREATE TRIGGER count_revision_statement AFTER INSERT ON core_blob_references REFERENCING NEW TABLE AS inserted_refs FOR EACH STATEMENT EXECUTE FUNCTION count_revision_statement()",
	] {
		sqlx::query(ddl).execute(&owner).await.unwrap();
	}
	let (status, patched) = request(
		&c.app, &token, "POST", &path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"note.md":null},"patch":"*** Begin Patch\n*** Add File: note.md\n+note\n*** End Patch"}),
	).await;
	assert_eq!(status, 200, "{patched}");
	let attempted: i64 =
		sqlx::query_scalar("SELECT count(*) FROM revision_write_probe WHERE kind = 'row'")
			.fetch_one(&owner)
			.await
			.unwrap();
	let statements: i64 =
		sqlx::query_scalar("SELECT count(*) FROM revision_write_probe WHERE kind = 'statement'")
			.fetch_one(&owner)
			.await
			.unwrap();
	assert_eq!(attempted, 5, "only the new manifest's rows are attempted");
	assert_eq!(
		statements, 1,
		"the new manifest is inserted in one statement"
	);
	c.close().await;
}
