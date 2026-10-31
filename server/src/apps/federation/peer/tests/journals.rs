//! Run invocations page in hundreds and keep their durable payloads whole.
use crate::endpoint::{EndpointFixture, assert_json, endpoint};
use aidash_server::apps::execution::models::{HumanRequest, Invocation};
use aidash_server::apps::execution::services::states::{HumanRequestKind, InvocationStatus};
use aidash_server::domain::NewTask;
use chrono::{Duration, Utc};
use reinhardt::db::orm::{Json, Model};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn run_invocations_page_in_hundreds_and_keep_durable_payloads(
	#[future] endpoint: EndpointFixture,
) {
	let app = Box::pin(endpoint).await;
	let mut db = app.database.lease.handle();
	let scope = crate::endpoint::tenant_scope(&app.runtime.store, "acme").await;
	let workspace = app
		.runtime
		.store
		.create_workspace(&scope, "alice", "Journal", "Bounded observation")
		.await
		.unwrap();
	crate::endpoint::register_fixture_agent(&app, "agent").await;
	let now = Utc::now();
	let mut observed_run = Uuid::nil();
	for (home, count) in [("aidash://journal-home", 101), ("aidash://other-home", 1)] {
		let task = app
			.runtime
			.store
			.create_task(
				&scope,
				workspace.id,
				&NewTask {
					title: home.into(),
					description: "Journal fixture".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"human",
				None,
			)
			.await
			.unwrap();
		// A Subject reads only Runs it may see at their home, so the observed Run is
		// a local one; the journal keys keep the fixture's home name.
		let run = if count > 1 {
			let agent = app
				.runtime
				.registry
				.get(
					&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
					"agent",
					"1.0.0",
				)
				.await
				.unwrap();
			crate::endpoint::admitted_run(&app.runtime, "acme", &task, &agent).await
		} else {
			crate::endpoint::remote_run(&app.runtime.store, &task, home, "agent", "1.0.0").await
		};
		if count > 1 {
			observed_run = run.id;
		}
		for index in 0..count {
			let invocation = Invocation::build()
				.tenant("acme")
				.idempotency_key(format!("{home}:{index:03}"))
				.run(run.id)
				.tool("bounded-fixture")
				.input(Json(json!({"content":"x".repeat(4096)})))
				.status(InvocationStatus::Completed)
				.result(Some(Json(json!({"content":"y".repeat(4096)}))))
				.replay_safe(true)
				.delivery_failures(0)
				.delivery_pending(false)
				.uncertain_reason(None)
				.created_at(now + Duration::milliseconds(index))
				.finish();
			Invocation::objects()
				.create_with_conn(&mut db, &invocation)
				.await
				.unwrap();
		}
		let human = HumanRequest::build()
			.tenant("acme")
			.id(Uuid::new_v4())
			.workspace_id(workspace.id)
			.run(run.id)
			.kind(HumanRequestKind::Question)
			.prompt("p".repeat(4096))
			.response(Some(Json(json!({"content":"r".repeat(4096)}))))
			.request_key(format!("question:{home}"))
			.answered_by(None::<String>)
			.finish();
		HumanRequest::objects()
			.create_with_conn(&mut db, &human)
			.await
			.unwrap();
	}
	let (member, _) = crate::endpoint::tenant_member(&app).await;
	let first = assert_json(
		member
			.get(&format!("/api/runs/{observed_run}"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(first["invocations"].as_array().unwrap().len(), 100);
	assert_eq!(
		first["invocations"][0]["idempotency_key"],
		"aidash://journal-home:000"
	);
	let next = assert_json(
		member
			.get(&format!("/api/runs/{observed_run}?offset=100"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(next["invocations"].as_array().unwrap().len(), 1);
	assert_eq!(
		next["invocations"][0]["idempotency_key"],
		"aidash://journal-home:100"
	);
	let durable = Invocation::objects()
		.filter(Invocation::field_idempotency_key().eq("aidash://journal-home:000"))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(durable.input["content"].as_str().unwrap().len(), 4096);
	assert_eq!(
		durable.result.unwrap()["content"].as_str().unwrap().len(),
		4096
	);
}
