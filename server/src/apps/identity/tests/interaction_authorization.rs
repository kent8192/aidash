#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::domain::{Run, qualified_agent};
use common::*;
use common::{TestEnvironment, test_environment};
use serde_json::{Value, json};
use uuid::Uuid;

fn conversation() -> Value {
	json!({"title":"Scoped conversation","goal":"Complete approved work","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"})
}

#[rstest::rstest]
#[tokio::test]
async fn conversation_admission_is_atomic_and_records_denials_without_orphans(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = common::application(f.clone()).await;
	let (mut policy, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) =
		request(&app, &token, "POST", "/api/conversations", conversation()).await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run: Run = {
		let query_bind_1 = workspace.parse::<Uuid>().unwrap();
		aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(created["task"]["status"], "CLAIMED");
	assert_eq!(created["delegation"]["delivered"], true);
	let chain: Vec<String> = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("subject_chain")),
				))
				.from(reinhardt::query::Alias::new("authorization_execution"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		chain,
		vec![
			"alice".to_owned(),
			qualified_agent(&f.config.node_id, "research", "1.0.0")
		]
	);
	let sender: String = {
		let query_bind_1 = run.workspace_id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("sender")),
				))
				.from(reinhardt::query::Alias::new("messages"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(sender, "alice");
	let before:Value=sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("JSONB_BUILD_ARRAY((SELECT COUNT(*) FROM workspaces), (SELECT COUNT(*) FROM authorization_workspaces), (SELECT COUNT(*) FROM tasks), (SELECT COUNT(*) FROM conversations), (SELECT COUNT(*) FROM runs), (SELECT COUNT(*) FROM messages), (SELECT COUNT(*) FROM events))")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(f.store.pool.driver()).await.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-execution","effect":"deny","subjects":{"any":true},"actions":["task.execute"],"resources":{"kinds":["task"]}}));
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
	assert_eq!(
		request(&app, &token, "POST", "/api/conversations", conversation())
			.await
			.0,
		403
	);
	let after:Value=sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("JSONB_BUILD_ARRAY((SELECT COUNT(*) FROM workspaces), (SELECT COUNT(*) FROM authorization_workspaces), (SELECT COUNT(*) FROM tasks), (SELECT COUNT(*) FROM conversations), (SELECT COUNT(*) FROM runs), (SELECT COUNT(*) FROM messages), (SELECT COUNT(*) FROM events))")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(f.store.pool.driver()).await.unwrap();
	assert_eq!(
		before, after,
		"a late admission denial must roll back the entire conversation"
	);
	let denied: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("authorization_decisions"))
			.and_where(reinhardt::query::Expr::cust(
				"action = 'task.execute' AND decision ->> 'reason' = 'explicit_deny'",
			))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		denied, 2,
		"root and agent rejection decisions must survive the rollback"
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn human_interactions_enforce_tenant_actions_read_visibility_and_actor_attribution(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use aidash_server::harness::Harness;
	let (f, url, schema) = setup(&_test_environment).await;
	let app = common::application(f.clone()).await;
	let (mut policy, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) =
		request(&app, &token, "POST", "/api/conversations", conversation()).await;
	assert_eq!(status, 200);
	let workspace = created["workspace"]["id"]
		.as_str()
		.unwrap()
		.parse::<Uuid>()
		.unwrap();
	let run: Run = {
		let query_bind_1 = workspace;
		aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	let human = f
		.store
		.human_request(
			&run,
			"APPROVAL_REQUIRED",
			"private approval prompt",
			"approval-1",
		)
		.await
		.unwrap();
	let mut other = policy.clone();
	other["tenant"] = json!("other");
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/other",
			json!({"expected_revision":0,"bundle":other})
		)
		.await
		.0,
		200
	);
	let (_, credential) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/other/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let foreign = credential["token"].as_str().unwrap();
	for (path, body) in [
		(
			format!("/api/runs/{}/message", run.id),
			json!({"content":"foreign message"}),
		),
		(
			format!("/api/human-requests/{}/answer", human.id),
			json!({"approved":true}),
		),
		(
			format!("/api/tasks/{}/abandon", run.task_id),
			json!({"revision":1,"reason":"foreign"}),
		),
	] {
		assert_eq!(request(&app, foreign, "POST", &path, body).await.0, 403);
	}
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/message", Uuid::new_v4()),
			json!({"content":"hidden run"})
		)
		.await
		.0,
		403,
		"an inaccessible or missing run stays hidden before peer preflight"
	);
	let mut concurrent = tokio::task::JoinSet::new();
	for index in 0..16 {
		let app = app.clone();
		let token = token.clone();
		let run_id = run.id;
		concurrent.spawn(async move {
			request(
				&app,
				&token,
				"POST",
				&format!("/api/runs/{run_id}/message"),
				json!({"content":format!("parallel scoped message {index}"),"idempotency_key":Uuid::new_v4()}),
			)
			.await
		});
	}
	let statuses = tokio::time::timeout(std::time::Duration::from_secs(20), async {
		let mut statuses = Vec::new();
		while let Some(result) = concurrent.join_next().await {
			statuses.push(result.unwrap().0);
		}
		statuses
	})
	.await
	.expect("concurrent scoped messages must not exhaust the access pool");
	assert_eq!(statuses, vec![200; 16]);
	let replay_key = Uuid::new_v4();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/message", run.id),
			json!({"content":"authorized message","idempotency_key":replay_key})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/message", run.id),
			json!({"content":"authorized message","idempotency_key":replay_key})
		)
		.await
		.0,
		200,
		"an authorized exact key replay uses the existing ledger row"
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/message", run.id),
			json!({"content":"changed content","idempotency_key":replay_key})
		)
		.await
		.0,
		409
	);
	let sender: String = {
		let query_bind_1 = workspace;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("sender")),
				))
				.from(reinhardt::query::Alias::new("messages"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ? AND content = 'authorized message')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(sender, "alice");
	let (status, answer) = request(
		&app,
		&token,
		"POST",
		&format!("/api/human-requests/{}/answer", human.id),
		json!({"approved":false}),
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(answer["answered_by"], "alice");
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/human-requests/{}/answer", human.id),
			json!({"approved":false})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/human-requests/{}/answer", human.id),
			json!({"approved":true})
		)
		.await
		.0,
		409
	);
	let answered: i64 = {
		let query_bind_1 = workspace;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("events"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ? AND kind = 'human.answered')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(answered, 1);
	let second = f
		.store
		.human_request(&run, "QUESTION", "another private prompt", "question-2")
		.await
		.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-interaction","effect":"deny","subjects":{"ids":["alice"]},"actions":["human.answer","run.message","task.abandon"],"resources":{"kinds":["*"]}}));
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
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/human-requests/{}/answer", second.id),
			json!({"text":"forbidden answer"})
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/message", run.id),
			json!({"content":"forbidden message"})
		)
		.await
		.0,
		403
	);
	let response: Option<Value> = {
		let query_bind_1 = second.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("response")),
				))
				.from(reinhardt::query::Alias::new("human_requests"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert!(response.is_none());
	let count: i64 = {
		let query_bind_1 = workspace;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("messages"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ? AND content = 'forbidden message')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(count, 0);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-interaction-read","effect":"deny","subjects":{"any":true},"actions":["human.read","conversation.read"],"resources":{"kinds":["*"]}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	// A journal can contain copied human prompts. Denying the source also hides
	// that run's aggregate data instead of exposing it through its context.
	{ let query_bind_1 = run.id; let query_bind_2 = common::pending(aidash_server::domain::RunState::Waiting(Box::new(aidash_server::domain::WaitingState::Human { request_id: human.id, resume: aidash_server::domain::ResumeState::Thinking(Default::default()) }))); let query_bind_3 = common::context(json!({"history":[{"seq":1,"event":{"kind":"human","request":"private approval prompt","request_kind":"APPROVAL","response":null}}],"journal":{"head":1,"imported_through":0,"inferred_through":0}})); sqlx::query(&reinhardt::query::Query::update().table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("phase"), reinhardt::query::Expr::cust("'WAITING'")).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("context"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).and_where(SimpleExpr::CustomWithExpr("(id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder)).execute(f.store.pool.driver()).await }.unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!("/api/runs/{}", run.id),
			Value::Null
		)
		.await
		.0,
		403
	);
	for path in [
		"/api/state".to_owned(),
		format!("/api/events?workspace_id={workspace}"),
		format!("/api/workspaces/{workspace}"),
	] {
		let (status, value) = request(&app, &token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200);
		assert!(
			!value.to_string().contains("private approval prompt"),
			"leak through {path}"
		);
		assert!(
			!value.to_string().contains("another private prompt"),
			"leak through {path}"
		);
		if path == "/api/state" {
			assert_eq!(value["conversations"], json!([]));
			assert_eq!(value["human_requests"], json!([]));
			assert_eq!(value["runs"], json!([]));
		}
	}
	Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let paused = f.store.run(run.id).await.unwrap();
	assert_eq!(paused.control.as_str(), "PAUSED");
	assert_eq!(paused.phase().as_str(), "WAITING");
	assert_eq!(
		json!(paused.state)["data"]["request_id"],
		human.id.to_string()
	);
	let task = f.store.task(run.task_id).await.unwrap();
	let failed = f
		.store
		.transition(
			task.id,
			task.revision,
			task.owner.as_deref().unwrap(),
			aidash_server::domain::TaskStatus::Failed,
		)
		.await
		.unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{}/abandon", task.id),
			json!({"revision":failed.revision,"reason":"stop failed child"})
		)
		.await
		.0,
		403
	);
	policy["policies"].as_array_mut().unwrap().truncate(1);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let (status, abandoned) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{}/abandon", task.id),
		json!({"revision":failed.revision,"reason":"stop failed child"}),
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(abandoned["status"], "ABANDONED");
	let actor: String = {
		let query_bind_1 = workspace;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("data ->> 'actor'"))
				.from(reinhardt::query::Alias::new("events"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ? AND kind = 'task.abandoned')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(actor, "alice");
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn cluster_conversations_recheck_approval_at_worker_boundaries(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = common::application(f.clone()).await;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let cluster = json!({"id":"cluster","version":"1.0.0","kind":"cluster","name":{"en":"cluster"},"description":{"en":"fixture"},"schema":{},"config":{"coordinator":{"id":"research","version":"1.0.0"}}});
	assert_eq!(
		request(&app, &f.config.api_token, "POST", "/api/registry", cluster)
			.await
			.0,
		200
	);
	let mut input = conversation();
	input["target"]["id"] = json!("cluster");
	input["target_kind"] = json!("cluster");
	assert_eq!(
		request(&app, &token, "POST", "/api/conversations", input.clone())
			.await
			.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"cluster","version":"1.0.0"},"expected_revision":0,"enabled":true})
		)
		.await
		.0,
		200
	);
	let (status, created) = request(&app, &token, "POST", "/api/conversations", input).await;
	assert_eq!(status, 200, "{created}");
	let task = created["task"]["id"]
		.as_str()
		.unwrap()
		.parse::<Uuid>()
		.unwrap();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"cluster","version":"1.0.0"},"expected_revision":1,"enabled":false})
		)
		.await
		.0,
		200
	);
	aidash_server::harness::Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == task)
		.unwrap();
	assert_eq!(run.control.as_str(), "PAUSED");
	assert_eq!(run.phase().as_str(), "READY");
	assert_eq!(f.store.task(task).await.unwrap().status.as_str(), "CLAIMED");
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;
