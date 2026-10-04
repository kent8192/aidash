//! Peer observations isolate the home node and bound durable journal payloads.
use crate::isolated;

use crate::endpoint::{EndpointFixture, assert_json, endpoint};
use aidash_server::apps::execution::models::{HumanRequest, Invocation};
use aidash_server::apps::execution::services::states::{HumanRequestKind, InvocationStatus};
use aidash_server::apps::federation::peer::models::Peer;
use aidash_server::domain::NewTask;
use chrono::{Duration, Utc};
use reinhardt::db::orm::{Json, Model};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

const PEER_ENV: &str = "AIDASH_SECRET_JOURNAL_FIXTURE";
const PEER_SECRET: &str = "local-journal-fixture-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

#[rstest]
#[tokio::test]
async fn peer_journals_bound_previews_and_exclude_other_home_nodes(
	#[future] endpoint: EndpointFixture,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	let app = Box::pin(endpoint).await;
	let mut db = app.database.lease.handle();
	let peer = Peer::build()
		.node_id("aidash://journal-home")
		.endpoint("http://127.0.0.1:1")
		.credential_env(PEER_ENV)
		.protocol_version("0.1")
		.enabled(true)
		.finish();
	Peer::objects()
		.create_with_conn(&mut db, &peer)
		.await
		.unwrap();
	let workspace = app
		.runtime
		.store
		.create_workspace("Journal", "Bounded observation")
		.await
		.unwrap();
	let now = Utc::now();
	let mut observed_run = Uuid::nil();
	for (home, count) in [("aidash://journal-home", 101), ("aidash://other-home", 1)] {
		let task = app
			.runtime
			.store
			.create_task(
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
		let run = app
			.runtime
			.store
			.accept_run(&task, home, "agent", "1.0.0")
			.await
			.unwrap();
		if count > 1 {
			observed_run = run.id;
		}
		for index in 0..count {
			let invocation = Invocation::build()
				.idempotency_key(format!("{home}:{index:03}"))
				.run_id(run.id)
				.tool("bounded-fixture")
				.input(Json(json!({"content":"x".repeat(4096)})))
				.status(InvocationStatus::Completed)
				.result(Some(Json(json!({"content":"y".repeat(4096)}))))
				.replay_safe(true)
				.created_at(now + Duration::milliseconds(index))
				.finish();
			Invocation::objects()
				.create_with_conn(&mut db, &invocation)
				.await
				.unwrap();
		}
		let human = HumanRequest::build()
			.id(Uuid::new_v4())
			.workspace_id(workspace.id)
			.run_id(run.id)
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
	app.anonymous
		.set_header("Authorization", &format!("Bearer {PEER_SECRET}"))
		.await
		.unwrap();
	app.anonymous
		.set_header("x-aidash-node", "aidash://journal-home")
		.await
		.unwrap();
	app.anonymous
		.set_header("x-aidash-protocol", "0.1")
		.await
		.unwrap();
	let journal = assert_json(
		app.anonymous.get("/federation/v0.1/observe").await.unwrap(),
		200,
	);
	assert_eq!(journal["node_id"], app.runtime.config.node_id);
	assert_eq!(journal["runs"].as_array().unwrap().len(), 1);
	assert_eq!(journal["runs"][0]["id"], observed_run.to_string());
	assert!(journal["runs"][0]["context"].is_null());
	assert!(journal["runs"][0]["state"].is_null());
	assert!(journal["runs"][0]["recovery"].is_null());
	assert!(journal["runs"][0]["state_error"].is_string());
	assert_eq!(journal["runs"][0]["state_version"], 1);
	assert_eq!(journal["human_requests"].as_array().unwrap().len(), 1);
	assert_eq!(
		journal["human_requests"][0]["prompt"]
			.as_str()
			.unwrap()
			.len(),
		1024
	);
	assert_eq!(journal["human_requests"][0]["response"]["truncated"], true);
	let invocations = journal["invocations"].as_array().unwrap();
	assert_eq!(invocations.len(), 100);
	assert_eq!(
		invocations[0]["idempotency_key"],
		"aidash://journal-home:100"
	);
	assert!(
		invocations
			.iter()
			.all(|item| item["run_id"] == observed_run.to_string()
				&& item["input"]["truncated"] == true
				&& item["result"]["truncated"] == true
				&& item["input"]["preview"].as_str().unwrap().len() == 1024)
	);
	let first = assert_json(
		app.operator
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
		app.operator
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
