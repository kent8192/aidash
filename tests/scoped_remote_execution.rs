mod common;
use aidash::{
	api,
	domain::{NewTask, qualified_agent},
	federation::{Federation, Home},
	harness::Harness,
	registry::EntityRef,
};
use axum::{
	Json, Router,
	body::{Body, to_bytes},
	extract::{Request, State},
	middleware::{self, Next},
	response::{IntoResponse, Response},
	routing::post,
};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Clone, Default)]
struct ModelScript {
	requests: Arc<Mutex<Vec<Value>>>,
	hold: Arc<AtomicBool>,
	entered: Arc<Notify>,
	release: Arc<Notify>,
}
struct Pair {
	model: ModelScript,
	drop_reply: Arc<Mutex<Option<String>>>,
	_environment: Arc<TestEnvironment>,
	a: Federation,
	b: Federation,
	aa: Router,
	ba: Router,
	source_policy: Value,
	receiver_policy: Value,
	token: String,
	task: Uuid,
	grant: Uuid,
	admission: Uuid,
	requests: Arc<Mutex<Vec<Value>>>,
	servers: Vec<tokio::task::JoinHandle<()>>,
	au: String,
	bu: String,
	aschema: String,
	bschema: String,
}
impl Pair {
	async fn close(self) {
		for server in self.servers {
			server.abort();
			let _ = server.await;
		}
		cleanup(self.a, &self.au, &self.aschema).await;
		cleanup(self.b, &self.bu, &self.bschema).await;
	}
	fn activation(&self) -> String {
		format!(
			"/api/tasks/{}/remote-grants/{}/activate",
			self.task, self.grant
		)
	}
	async fn run(&self) -> aidash::domain::Run {
		self.b.store.run(self.admission).await.unwrap()
	}
	async fn step(&self) {
		let worker = Harness {
			federation: self.b.clone(),
		};
		tokio::time::timeout(std::time::Duration::from_secs(15), async {
			loop {
				if worker.worker_once().await.unwrap() {
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(100)).await;
			}
		})
		.await
		.expect("scoped step must progress within its bounded retry delay");
	}
}

fn source_router(f: &Federation, state: Arc<Mutex<Option<String>>>) -> Router {
	api::router(f.clone()).layer(middleware::from_fn_with_state(state, lose_command_reply))
}
async fn lose_command_reply(
	State(state): State<Arc<Mutex<Option<String>>>>,
	request: Request,
	next: Next,
) -> Response {
	if !request.uri().path().ends_with("/scoped/execution/commands") {
		return next.run(request).await;
	}
	let (parts, body) = request.into_parts();
	let bytes = to_bytes(body, 4 * 1024 * 1024).await.unwrap();
	let input: Value = serde_json::from_slice(&bytes).unwrap();
	let drop = {
		let mut wanted = state.lock().await;
		if wanted
			.as_deref()
			.is_some_and(|value| input["operation"] == value)
		{
			wanted.take();
			true
		} else {
			false
		}
	};
	let response = next
		.run(Request::from_parts(parts, Body::from(bytes)))
		.await;
	if drop && response.status().is_success() {
		(
			axum::http::StatusCode::SERVICE_UNAVAILABLE,
			Json(json!({"error":"fixture lost a committed reply"})),
		)
			.into_response()
	} else {
		response
	}
}
async fn reconnect(f: &mut Federation) {
	let pool = f
		.store
		.pool
		.options()
		.clone()
		.connect_with(f.store.pool.connect_options().as_ref().clone())
		.await
		.unwrap();
	let store = aidash::store::Store::from_pool(pool, f.config.node_id.clone())
		.await
		.unwrap();
	f.registry = aidash::registry::Registry::new(store.pool.clone(), &f.config.node_id);
	f.store.pool.close().await;
	f.store.control_pool.close().await;
	f.store = store;
	f.client = reqwest::Client::new();
	f.notify = Arc::new(Notify::new());
}

#[rstest::fixture]
async fn scoped_pair(#[future(awt)] test_environment: Arc<TestEnvironment>) -> Pair {
	let _ = tracing_subscriber::fmt()
		.with_env_filter("aidash=debug")
		.with_test_writer()
		.try_init();
	let (mut a, au, aschema) = setup(&test_environment).await;
	let (mut b, bu, bschema) = setup(&test_environment).await;
	b.config.node_id = "aidash://scoped-receiver".into();
	b.store.node_id = b.config.node_id.clone();
	let model = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", model.local_addr().unwrap());
	let model_state = ModelScript::default();
	let requests = model_state.requests.clone();
	let model_app=Router::new().route("/v1/chat/completions",post(|State(script):State<ModelScript>,Json(input):Json<Value>|async move {
        let mut calls=script.requests.lock().await;
        calls.push(input);
        let count=calls.len(); drop(calls); script.entered.notify_one(); if script.hold.load(Ordering::Acquire) {script.release.notified().await;} let message=if count==1 {json!({"role":"assistant","content":null,"tool_calls":[{"id":"note","type":"function","function":{"name":"workspace_message","arguments":"{\"content\":\"Scoped remote progress\"}"}}]})} else {json!({"role":"assistant","content":"Scoped remote result"})};
        Json(json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    })).with_state(model_state.clone());
	let model_server = tokio::spawn(async move { axum::serve(model, model_app).await.unwrap() });
	let al = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let bl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	a.config.endpoint = format!("http://{}", al.local_addr().unwrap());
	b.config.endpoint = format!("http://{}", bl.local_addr().unwrap());
	let drop_reply = Arc::new(Mutex::new(None));
	let aa = source_router(&a, drop_reply.clone());
	let ba = api::router(b.clone());
	let (mut source_policy, token, task) = bootstrap(&a, &aa, &endpoint).await;
	let (receiver_policy, _, _) = bootstrap(&b, &ba, &endpoint).await;
	source_policy["subjects"][qualified_agent(&b.config.node_id, "research", "1.0.0")] =
		json!({"kind":"agent"});
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":source_policy})
		)
		.await
		.0,
		200
	);
	for (local, other) in [(&a, &b), (&b, &a)] {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("peers"))
				.columns(
					[
						"node_id",
						"endpoint",
						"credential_env",
						"protocol_version",
						"enabled",
					]
					.map(Alias::new),
				)
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
					Expr::cust("'0.1'"),
					Expr::cust("TRUE"),
				])
				.to_string(PostgresQueryBuilder),
		)
		.bind(&other.config.node_id)
		.bind(&other.config.endpoint)
		.execute(&local.store.pool)
		.await
		.unwrap();
	}
	let (_, credential) = request(
		&ba,
		&b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(request(&ba,&b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":credential["credential"]["id"],"enabled":true,"expected_revision":0})).await.0,200);
	let aapp = aa.clone();
	let bapp = ba.clone();
	let aserver = tokio::spawn(async move { axum::serve(al, aapp).await.unwrap() });
	let bserver = tokio::spawn(async move { axum::serve(bl, bapp).await.unwrap() });
	let grant = Uuid::new_v4();
	let (status,prepared)=request(&aa,&token,"POST",&format!("/api/tasks/{task}/remote-grants"),json!({"id":grant,"node_id":b.config.node_id,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300})).await;
	assert_eq!(status, 200, "{prepared}");
	let (status, activated) = request(
		&aa,
		&token,
		"POST",
		&format!("/api/tasks/{task}/remote-grants/{grant}/activate"),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{activated}");
	let admission = serde_json::from_value(activated["admission_id"].clone()).unwrap();
	Pair {
		model: model_state,
		drop_reply,
		_environment: test_environment,
		a,
		b,
		aa,
		ba,
		source_policy,
		receiver_policy,
		token,
		task,
		grant,
		admission,
		requests,
		servers: vec![model_server, aserver, bserver],
		au,
		bu,
		aschema,
		bschema,
	}
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_remote_worker_finishes_at_home_and_retries_keep_one_execution(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	let (status, replay) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay["run_id"], p.admission.to_string());
	for _ in 0..10 {
		if p.run().await.phase == "COMPLETED" {
			break;
		}
		p.step().await;
		let run = p.run().await;
		assert_eq!(run.control, "ACTIVE", "{} {:?}", run.phase, run.error);
		assert!(run.error.is_none(), "{} {:?}", run.phase, run.error);
	}
	let run = p.run().await;
	assert_eq!(run.phase, "COMPLETED", "{:?}", run.error);
	assert_eq!(run.agent_version, "1.0.0");
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(task.status, "COMPLETED");
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(snapshot.artifacts[0].content, "Scoped remote result");
	assert_eq!(
		snapshot
			.messages
			.iter()
			.filter(|m| m.content == "Scoped remote progress")
			.count(),
		1
	);
	let events =
		p.a.store
			.events(0, Some(task.workspace_id), 1000)
			.await
			.unwrap();
	let completed = events
		.iter()
		.filter(|event| event.kind == "task.remote_tool_completed")
		.collect::<Vec<_>>();
	assert!(!completed.is_empty());
	assert!(
		completed
			.iter()
			.all(|event| event.data["remote_run_id"] == json!(p.admission)
				&& event.data["detail"]["call"]["name"].is_string())
	);
	assert!(
		completed
			.iter()
			.all(|event| event.data["detail"]["call"]["arguments"].is_null())
	);
	assert!(
		!events
			.iter()
			.any(|event| event.kind == "task.remote_run_recovered")
	);
	assert_eq!(p.requests.lock().await.len(), 2);
	let input = p
		.requests
		.lock()
		.await
		.iter()
		.map(ToString::to_string)
		.collect::<String>();
	assert!(!input.contains(&p.token));
	assert!(!input.contains(&std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()));
	let (status, replay) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay["phase"], "COMPLETED");
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_remote_agent_can_delegate_its_created_child_to_the_home_node(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	let run = p.run().await;
	let home = Home::new(p.b.clone(), run);
	let child = home
		.create_task(
			"scoped-child-create",
			&NewTask {
				title: "Scoped delegated child".into(),
				description: "Created under a remote run and delegated home".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: Some(p.task),
			},
		)
		.await
		.unwrap();
	let agent = EntityRef {
		id: "research".into(),
		version: "1.0.0".into(),
	};
	let delegation = home
		.delegate_with_key(
			"scoped-child-delegate",
			child.id,
			&p.a.config.node_id,
			&agent,
		)
		.await
		.unwrap();
	assert_eq!(delegation.task_id, child.id);
	assert_eq!(delegation.node_id, p.a.config.node_id);
	assert_eq!(delegation.agent_id, agent.id);
	assert_eq!(delegation.agent_version, agent.version);
	assert!(delegation.delivered);

	let rows: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("runs"))
			.and_where(Expr::col(Alias::new("task_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("home_node")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(child.id)
	.bind(&p.a.config.node_id)
	.fetch_one(&p.a.store.pool)
	.await
	.unwrap();
	assert_eq!(rows, 1, "a retry must not create a second delegated Run");
	let replay = home
		.delegate_with_key(
			"scoped-child-delegate",
			child.id,
			&p.a.config.node_id,
			&agent,
		)
		.await
		.unwrap();
	assert_eq!(replay.task_id, child.id);
	assert_eq!(
		p.a.store
			.runs()
			.await
			.unwrap()
			.into_iter()
			.filter(|run| run.task_id == child.id && run.home_node == p.a.config.node_id)
			.count(),
		1
	);
	p.close().await;
}

#[rstest::rstest]
#[case("source_policy")]
#[case("receiver_policy")]
#[case("expiry")]
#[case("grant_revocation")]
#[case("source_credential")]
#[case("receiver_mapping")]
#[case("task_revision")]
#[case("disabled_agent")]
#[case("definition_substitution")]
#[tokio::test]
async fn a_changed_execution_boundary_stops_before_inference(
	#[case] fault: &str,
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	assert_eq!(p.run().await.phase, "THINKING");
	match fault {
		"source_policy" | "receiver_policy" => {
			let (f, app, mut bundle, revision) = if fault == "source_policy" {
				(&p.a, &p.aa, p.source_policy.clone(), 2)
			} else {
				(&p.b, &p.ba, p.receiver_policy.clone(), 1)
			};
			bundle["policies"].as_array_mut().unwrap().push(json!({"id":"deny-model","effect":"deny","subjects":{"any":true},"actions":["model.infer"],"resources":{"kinds":["model"]}}));
			assert_eq!(
				request(
					app,
					&f.config.api_token,
					"POST",
					"/api/authorization/acme",
					json!({"expected_revision":revision,"bundle":bundle})
				)
				.await
				.0,
				200
			);
		}
		"expiry" => {
			sqlx::query(
				&Query::update()
					.table(Alias::new("authorization_remote_grants"))
					.value(
						Alias::new("expires_at"),
						Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
					)
					.and_where(Expr::cust("id=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(p.grant)
			.execute(&p.a.store.pool)
			.await
			.unwrap();
		}
		"grant_revocation" => {
			assert_eq!(
				request(
					&p.aa,
					&p.token,
					"POST",
					&format!("/api/tasks/{}/remote-grants/{}/revoke", p.task, p.grant),
					json!({})
				)
				.await
				.0,
				200
			);
		}
		"source_credential" => {
			let id: Uuid = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("credential_id"))
					.from(Alias::new("authorization_remote_grants"))
					.and_where(Expr::cust("id=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(p.grant)
			.fetch_one(&p.a.store.pool)
			.await
			.unwrap();
			assert_eq!(
				request(
					&p.aa,
					&p.a.config.api_token,
					"POST",
					&format!("/api/authorization/acme/credentials/{id}/revoke"),
					json!({})
				)
				.await
				.0,
				200
			);
		}
		"receiver_mapping" => {
			let id: Uuid = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("credential_id"))
					.from(Alias::new("authorization_peer_mappings"))
					.and_where(Expr::cust("source_node=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(&p.a.config.node_id)
			.fetch_one(&p.b.store.pool)
			.await
			.unwrap();
			assert_eq!(request(&p.ba,&p.b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":p.a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":id,"enabled":false,"expected_revision":1})).await.0,200);
		}
		"task_revision" => {
			sqlx::query(
				&Query::update()
					.table(Alias::new("tasks"))
					.value(Alias::new("revision"), Expr::cust("revision+1"))
					.and_where(Expr::cust("id=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(p.task)
			.execute(&p.a.store.pool)
			.await
			.unwrap();
		}
		"disabled_agent" => {
			assert_eq!(
				request(
					&p.ba,
					&p.b.config.api_token,
					"POST",
					"/api/authorization/acme/catalog",
					json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false})
				)
				.await
				.0,
				200
			);
		}
		"definition_substitution" => {
			sqlx::query(
				&Query::update()
					.table(Alias::new("runs"))
					.value(Alias::new("agent_version"), Expr::cust("'9.9.9'"))
					.and_where(Expr::cust("id=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(p.admission)
			.execute(&p.b.store.pool)
			.await
			.unwrap();
		}
		_ => panic!("unknown fault"),
	}
	p.step().await;
	let run = p.run().await;
	assert_eq!(
		run.control, "PAUSED",
		"{fault}: {} {:?}",
		run.phase, run.error
	);
	assert!(p.requests.lock().await.is_empty());
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(task.status, "RUNNING");
	assert!(
		p.a.store
			.snapshot(task.workspace_id)
			.await
			.unwrap()
			.artifacts
			.is_empty()
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_inputs_are_durable_and_idempotent_and_controls_remain_scoped(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	let path = format!("/api/tasks/{}/remote-grants/{}/messages", p.task, p.grant);
	let input =
		json!({"id":Uuid::new_v4(),"content":"Keep this accepted correction after reconnect."});
	let first = request(&p.aa, &p.token, "POST", &path, input.clone()).await;
	assert_eq!(first.0, 200, "{}", first.1);
	assert_eq!(
		first,
		request(&p.aa, &p.token, "POST", &path, input.clone()).await
	);
	let mut changed = input.clone();
	changed["content"] = json!("Cannot replace an admitted message");
	assert_eq!(
		request(&p.aa, &p.token, "POST", &path, changed).await.0,
		409
	);
	assert_eq!(p.b.store.run_inputs(p.admission).await.unwrap().len(), 1);
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(
		p.a.store
			.snapshot(task.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.filter(|m| m.content == input["content"])
			.count(),
		1
	);
	let control = format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant);
	assert_eq!(
		request(&p.aa, &p.token, "POST", &control, json!({"action":"pause"}))
			.await
			.0,
		200
	);
	assert_eq!(p.run().await.control, "PAUSED");
	let states = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/tasks/{}/remote-executions", p.task),
		json!({}),
	)
	.await;
	assert_eq!(states.0, 200, "{}", states.1);
	assert_eq!(states.1[0]["execution"]["control"], "PAUSED");
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&control,
			json!({"action":"resume"})
		)
		.await
		.0,
		200
	);
	for _ in 0..10 {
		if p.run().await.phase == "COMPLETED" {
			break;
		}
		p.step().await;
		let run = p.run().await;
		assert_eq!(run.control, "ACTIVE", "{} {:?}", run.phase, run.error);
		assert!(run.error.is_none(), "{:?}", run.error);
	}
	assert_eq!(p.run().await.phase, "COMPLETED");
	assert!(
		p.requests
			.lock()
			.await
			.iter()
			.any(|r| r.to_string().contains("Keep this accepted correction"))
	);
	assert_eq!(first, request(&p.aa, &p.token, "POST", &path, input).await);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn revocation_during_remote_inference_discards_response_and_cancel_closes_both_sides(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.model.hold.store(true, Ordering::Release);
	let worker = Harness {
		federation: p.b.clone(),
	};
	let step = tokio::spawn(async move { worker.worker_once().await.unwrap() });
	tokio::time::timeout(
		std::time::Duration::from_secs(10),
		p.model.entered.notified(),
	)
	.await
	.unwrap();
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&format!("/api/tasks/{}/remote-grants/{}/revoke", p.task, p.grant),
			json!({})
		)
		.await
		.0,
		200
	);
	p.model.release.notify_one();
	assert!(
		tokio::time::timeout(std::time::Duration::from_secs(10), step)
			.await
			.unwrap()
			.unwrap()
	);
	assert_eq!(p.run().await.control, "PAUSED");
	let task = p.a.store.task(p.task).await.unwrap();
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	assert!(snapshot.artifacts.is_empty());
	assert!(snapshot.messages.is_empty());
	let control = format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant);
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&control,
			json!({"action":"resume"})
		)
		.await
		.0,
		403
	);
	let cancelled = request(
		&p.aa,
		&p.token,
		"POST",
		&control,
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(cancelled.0, 200, "{}", cancelled.1);
	p.step().await;
	assert_eq!(p.run().await.phase, "CANCELLED");
	assert_eq!(p.a.store.task(p.task).await.unwrap().status, "CANCELLED");
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&control,
			json!({"action":"cancel"})
		)
		.await
		.0,
		200
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn peer_outage_and_both_node_restarts_reconcile_one_scoped_execution(
	#[future(awt)] scoped_pair: Pair,
) {
	let mut p = scoped_pair;
	p.step().await;
	for server in p.servers.drain(1..) {
		server.abort();
		let _ = server.await;
	}
	p.step().await;
	assert!(p.run().await.pending["retry_at"].is_string());
	assert!(p.run().await.error.is_some());
	assert!(p.requests.lock().await.is_empty());
	assert!(
		p.a.store
			.snapshot(p.a.store.task(p.task).await.unwrap().workspace_id)
			.await
			.unwrap()
			.artifacts
			.is_empty()
	);
	reconnect(&mut p.a).await;
	reconnect(&mut p.b).await;
	p.aa = source_router(&p.a, p.drop_reply.clone());
	p.ba = api::router(p.b.clone());
	for (f, app) in [(&p.a, p.aa.clone()), (&p.b, p.ba.clone())] {
		let endpoint = reqwest::Url::parse(&f.config.endpoint).unwrap();
		let listener = tokio::net::TcpListener::bind(("127.0.0.1", endpoint.port().unwrap()))
			.await
			.unwrap();
		p.servers.push(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap()
		}));
	}
	let replay = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(replay.0, 200, "{}", replay.1);
	assert_eq!(replay.1["run_id"], p.admission.to_string());
	assert_eq!(
		replay.1["control"], "ACTIVE",
		"activation preserves the durable control state"
	);
	let resumed = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(resumed.0, 200, "{}", resumed.1);
	for _ in 0..10 {
		if p.run().await.phase == "COMPLETED" {
			break;
		}
		p.step().await;
	}
	assert_eq!(p.run().await.phase, "COMPLETED");
	let runs = p.b.store.runs().await.unwrap();
	assert_eq!(
		runs.iter()
			.filter(|run| run.home_node == p.a.config.node_id && run.task_id == p.task)
			.count(),
		1
	);
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::cust("grant_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(p.grant)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(count, 1);
	let snapshot =
		p.a.store
			.snapshot(p.a.store.task(p.task).await.unwrap().workspace_id)
			.await
			.unwrap();
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(
		snapshot
			.messages
			.iter()
			.filter(|message| message.content == "Scoped remote progress")
			.count(),
		1
	);
	assert_eq!(p.requests.lock().await.len(), 2);
	p.close().await;
}

#[rstest::rstest]
#[case("message")]
#[case("run_message_complete")]
#[tokio::test]
async fn lost_home_effect_reply_reconciles_without_duplicate_effects(
	#[case] operation: &str,
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	*p.drop_reply.lock().await = Some(operation.to_owned());
	for _ in 0..15 {
		if p.run().await.phase == "COMPLETED" {
			break;
		}
		p.step().await;
		let run = p.run().await;
		if run.control == "PAUSED" {
			let response = request(
				&p.aa,
				&p.token,
				"POST",
				&format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant),
				json!({"action":"resume"}),
			)
			.await;
			assert_eq!(response.0, 200, "{}", response.1);
		}
	}
	assert!(
		p.drop_reply.lock().await.is_none(),
		"the fault must be exercised"
	);
	assert_eq!(
		p.run().await.phase,
		"COMPLETED",
		"{:?}",
		p.run().await.error
	);
	let snapshot =
		p.a.store
			.snapshot(p.a.store.task(p.task).await.unwrap().workspace_id)
			.await
			.unwrap();
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(
		snapshot
			.messages
			.iter()
			.filter(|message| message.content == "Scoped remote progress")
			.count(),
		1
	);
	assert_eq!(
		p.requests.lock().await.len(),
		2,
		"completed inference is never replayed after a lost home reply"
	);
	p.close().await;
}
