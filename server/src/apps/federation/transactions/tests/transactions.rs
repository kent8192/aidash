#[path = "../../../execution/tests/support/legacy.rs"]
mod common;
#[path = "transactions/review.rs"]
mod review;

use chrono::{Duration, Utc};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

#[rstest::rstest]
#[case::accepted(false)]
#[case::agent_denied(true)]
#[tokio::test]
async fn paired_subject_completion_preserves_the_stored_execution_chain(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] denied: bool,
) {
	use aidash_server::transactions::{Manifest, coordinator, participant};
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let run = f.store.runs().await.unwrap().remove(0);
	let task = f.store.task(task_id).await.unwrap();
	let task = f
		.store
		.transition(
			task.id,
			task.revision,
			task.owner.as_deref().unwrap(),
			aidash_server::domain::TaskStatus::Running,
		)
		.await
		.unwrap();
	// The provider has returned a final answer; no external tool or worker lease
	// remains. The production claim above wrote the actual execution authority.
	{ let query_bind_1 = run.id; let query_bind_2 = common::tool_pending(json!({"response":{"text":"Atomic answer","tool_calls":[],"input_tokens":0,"output_tokens":0},"cursor":0})); sqlx::query(&Query::update().table(Alias::new("runs"))
		.value(Alias::new("phase"), "TOOL_CALL")
		.value_expr(Alias::new("pending"), Expr::value(query_bind_2.to_owned()))
		.and_where(SimpleExpr::CustomWithExpr("(id=?)".into(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(PostgresQueryBuilder))
		.execute(f.store.pool.driver()).await }.unwrap();
	if denied {
		let agent = aidash_server::domain::qualified_agent(&f.config.node_id, "research", "1.0.0");
		policy["policies"].as_array_mut().unwrap().push(json!({"id":"agent-finalize-denied","effect":"deny","subjects":{"ids":[agent]},"actions":["run.finish"],"resources":{"kinds":["run"]}}));
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":policy})
			)
			.await
			.0,
			200
		);
	}
	let manifest: Manifest = serde_json::from_value(json!({"id":Uuid::new_v4(),"coordinator":f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),
		"participants":[{"node_id":f.config.node_id,"mutations":[
			{"kind":"complete_task","task_id":task_id,"expected_revision":task.revision,"artifact":{"kind":"text","name":"Answer","content":"Atomic answer"}},
			{"kind":"finish_run","run_id":run.id,"task_id":task_id,"expected_revision":run.revision}]}]})).unwrap();
	let (status, body) = request(&app, &token, "POST", "/api/transactions", json!(manifest)).await;
	assert_eq!(status, if denied { 403 } else { 202 }, "{body}");
	if denied {
		assert!(matches!(
			coordinator::status(&f, manifest.id).await,
			Err(aidash_server::Error::NotFound(_))
		));
		assert_eq!(
			f.store.run(run.id).await.unwrap().phase().as_str(),
			"TOOL_CALL"
		);
		assert!(
			f.store
				.snapshot(task.workspace_id)
				.await
				.unwrap()
				.artifacts
				.is_empty()
		);
	} else {
		for _ in 0..12 {
			if coordinator::advance(&f, manifest.id)
				.await
				.unwrap()
				.complete
			{
				break;
			}
		}
		let outcome = coordinator::status(&f, manifest.id).await.unwrap();
		assert!(outcome.complete, "{outcome:?}");
		assert_eq!(outcome.decision.as_deref(), Some("COMMIT"), "{outcome:?}");
		for _ in 0..2 {
			participant::finish(&f, &f.config.node_id, &manifest)
				.await
				.unwrap();
		}
		assert_eq!(
			f.store.task(task_id).await.unwrap().status.as_str(),
			"COMPLETED"
		);
		assert_eq!(
			f.store.run(run.id).await.unwrap().phase().as_str(),
			"COMPLETED"
		);
		let artifacts = f.store.snapshot(task.workspace_id).await.unwrap().artifacts;
		assert_eq!(artifacts.len(), 1);
		assert_eq!(artifacts[0].created_by, task.owner.unwrap());
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::revoked_before_admission(0, "ABORT", 0)]
#[case::revoked_after_admission(1, "COMMIT", 1)]
#[tokio::test]
async fn subject_transaction_admission_preserves_only_accepted_obligations(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] steps_before_revocation: usize,
	#[case] decision: &str,
	#[case] revision: i64,
) {
	use aidash_server::transactions::{Manifest, coordinator};
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, subject, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let manifest: Manifest = serde_json::from_value(json!({
		"id": Uuid::new_v4(), "coordinator": f.config.node_id,
		"isolation": "serializable", "deadline": Utc::now() + Duration::minutes(5),
		"participants": [{"node_id": f.config.node_id, "mutations": [{
			"kind": "workspace_state", "workspace_id": workspace,
			"expected_revision": 0, "state": {"accepted": true}
		}]}]
	}))
	.unwrap();
	let (status, body) =
		request(&app, &subject, "POST", "/api/transactions", json!(manifest)).await;
	assert_eq!(status, 202, "{body}");
	for _ in 0..steps_before_revocation {
		coordinator::advance(&f, manifest.id).await.unwrap();
	}
	let (_, credentials) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/authorization/acme/credentials",
		Value::Null,
	)
	.await;
	let credential = credentials
		.as_array()
		.unwrap()
		.iter()
		.find(|value| value["subject"] == "alice")
		.unwrap()["id"]
		.as_str()
		.unwrap();
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		&format!("/api/authorization/acme/credentials/{credential}/revoke"),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 200,
		"revocation must remain available during a reservation: {body}"
	);
	for _ in 0..12 {
		if coordinator::advance(&f, manifest.id)
			.await
			.unwrap()
			.complete
		{
			break;
		}
	}
	let outcome = coordinator::status(&f, manifest.id).await.unwrap();
	assert!(outcome.complete, "{outcome:?}");
	assert_eq!(outcome.decision.as_deref(), Some(decision));
	assert_eq!(
		f.store.workspace(workspace).await.unwrap().revision,
		revision
	);
	assert_eq!(
		request(
			&app,
			&subject,
			"GET",
			&format!("/api/transactions/{}", manifest.id),
			Value::Null
		)
		.await
		.0,
		401
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn atomic_submission_validates_the_entire_manifest_before_creating_work(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use aidash_server::{Error, transactions::coordinator};
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, subject, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (session_status, session) =
		request(&app, &subject, "GET", "/api/session", Value::Null).await;
	assert_eq!(session_status, 200);
	assert_eq!(
		session["access"],
		json!({"kind":"subject","tenant":"acme","subject":"alice"})
	);
	assert_eq!(
		request(&app, &subject, "GET", "/api/transactions", Value::Null)
			.await
			.0,
		200
	);
	let workspace = f
		.store
		.create_workspace("Atomic", "Commit together")
		.await
		.unwrap();
	let id = Uuid::new_v4();
	let manifest = json!({"id":id,"coordinator":f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),"participants":[{"node_id":f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":workspace.id,"expected_revision":0,"state":{"result":"committed"}}]}]});
	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/transactions",
		manifest.clone(),
	)
	.await;
	assert_eq!(status, 202, "{response}");
	assert_eq!(response["id"], json!(id));
	assert_eq!(response["decision"], Value::Null);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/transactions",
			manifest.clone()
		)
		.await,
		(202, response)
	);
	let mut changed = manifest.clone();
	changed["participants"][0]["mutations"][0]["state"] = json!({"different":true});
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/transactions",
			changed
		)
		.await
		.0,
		409
	);
	let mut external = manifest;
	external["id"] = json!(Uuid::new_v4());
	external["participants"][0]["mutations"]
		.as_array_mut()
		.unwrap()
		.push(json!({"kind":"external_tool","endpoint":"http://127.0.0.1:9/effect"}));
	// Native Json extraction rejects unknown mutations before transaction admission.
	let authorization = format!("Bearer {}", f.config.api_token);
	// reinhardt-web#6672: do not mix shared default and per-request credentials.
	let rejected = app
		.client()
		.post_raw_with_headers(
			"/api/transactions",
			external.to_string().as_bytes(),
			"application/json",
			&[("Authorization", authorization.as_str())],
		)
		.await
		.unwrap();
	assert_eq!(rejected.status_code(), 422, "{}", rejected.text());
	assert_eq!(rejected.content_type(), Some("text/plain; charset=utf-8"));
	assert!(
		rejected
			.text()
			.starts_with("Failed to deserialize the JSON body into the target type: ")
	);
	assert!(matches!(
		coordinator::status(
			&f,
			Uuid::parse_str(external["id"].as_str().unwrap()).unwrap()
		)
		.await,
		Err(Error::NotFound(_))
	));
	assert_eq!(
		f.store.workspace(workspace.id).await.unwrap().state,
		json!({})
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::transaction_action("transaction.submit")]
#[case::underlying_action("workspace.update")]
#[case::manifest_disclosure("transaction.disclose")]
#[tokio::test]
async fn subject_transaction_requires_all_authority_before_persisting_a_manifest(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] denied: &str,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-transaction", "effect":"deny","subjects":{"any":true},"actions":[denied],"resources":{"kinds":["*"]}}));
	let auth = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let revision = auth.snapshot("acme").await.unwrap().revision;
	auth.replace(
		"acme",
		revision,
		serde_json::from_value(policy).unwrap(),
		"operator",
	)
	.await
	.unwrap();
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let id = Uuid::new_v4();
	let manifest = json!({"id":id,"coordinator":f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),
        "participants":[{"node_id":f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":workspace,"expected_revision":0,"state":{"private":"do not disclose"}}]}]});
	let (status, body) = request(&app, &token, "POST", "/api/transactions", manifest).await;
	assert_eq!(status, 403, "{body}");
	assert!(matches!(
		aidash_server::transactions::coordinator::status(&f, id).await,
		Err(aidash_server::Error::NotFound(_))
	));
	assert_eq!(f.store.workspace(workspace).await.unwrap().revision, 0);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn busy_subject_submission_preserves_exact_id_retry_and_current_read_authority(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use aidash_server::transactions::coordinator;
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let id = Uuid::new_v4();
	let manifest = json!({"id":id,"coordinator":f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),
        "participants":[{"node_id":f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":workspace,"expected_revision":0,"state":{"accepted":true}}]}]});
	assert_eq!(
		request(&app, &token, "POST", "/api/transactions", manifest.clone())
			.await
			.0,
		202
	);
	coordinator::advance(&f, id).await.unwrap();
	assert_eq!(
		request(&app, &token, "POST", "/api/transactions", manifest.clone())
			.await
			.0,
		202
	);
	let mut other = manifest.clone();
	other["id"] = json!(Uuid::new_v4());
	assert_eq!(
		request(&app, &token, "POST", "/api/transactions", other)
			.await
			.0,
		503
	);
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!("/api/transactions/{id}"),
			Value::Null
		)
		.await
		.0,
		200
	);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-read","effect":"deny","subjects":{"any":true},"actions":["workspace.read"],"resources":{"kinds":["*"]}}));
	let auth = aidash_server::authorization::Authorization {
		pool: f.store.control_pool.clone(),
	};
	let revision = auth.snapshot("acme").await.unwrap().revision;
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":revision,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!("/api/transactions/{id}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(&app, &token, "GET", "/api/transactions", Value::Null)
			.await
			.1,
		json!([])
	);
	for _ in 0..12 {
		if coordinator::advance(&f, id).await.unwrap().complete {
			break;
		}
	}
	assert_eq!(
		coordinator::status(&f, id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("COMMIT")
	);
	assert_eq!(f.store.workspace(workspace).await.unwrap().revision, 1);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn authority_control_does_not_unlock_ordinary_mutations(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use aidash_server::transactions::{Manifest, coordinator};
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let manifest:Manifest=serde_json::from_value(json!({"id":Uuid::new_v4(),"coordinator":f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),
        "participants":[{"node_id":f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":workspace,"expected_revision":0,"state":{"accepted":true}}]}]})).unwrap();
	assert_eq!(
		request(&app, &token, "POST", "/api/transactions", json!(manifest))
			.await
			.0,
		202
	);
	coordinator::advance(&f, manifest.id).await.unwrap();
	// Authority metadata and audit can change, but neither HTTP nor a direct
	// Store mutation may use those control paths to publish application data.
	for operation in ["evaluate", "simulate"] {
		let (status,body)=request(&app,&f.config.api_token,"POST",&format!("/api/authorization/acme/{operation}"),
			json!({"subject":"alice","action":"transaction.read","resource":{"tenant":"acme","kind":"transaction","id":manifest.id.to_string()}})).await;
		assert_eq!(
			status, 200,
			"authority evaluation during the barrier: {body}"
		);
		assert_eq!(body["allowed"], true);
	}
	assert_eq!(
		request(
			&app,
			&token,
			"PATCH",
			&format!("/api/workspaces/{workspace}"),
			json!({"revision":0,"state":{"escape":true}})
		)
		.await
		.0,
		503
	);
	assert!(
		f.store
			.update_state(workspace, 0, json!({"escape":true}))
			.await
			.is_err()
	);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice","expires_in_seconds":3600})
		)
		.await
		.0,
		503
	);
	coordinator::abort(&f, manifest.id).await.unwrap();
	for _ in 0..8 {
		if coordinator::advance(&f, manifest.id)
			.await
			.unwrap()
			.complete
		{
			break;
		}
	}
	assert_eq!(f.store.workspace(workspace).await.unwrap().revision, 0);
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;
