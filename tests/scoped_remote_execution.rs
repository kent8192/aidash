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
use futures_util::StreamExt;
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Clone, Default)]
struct ModelScript {
	requests: Arc<Mutex<Vec<Value>>>,
	hold: Arc<AtomicBool>,
	entered: Arc<Notify>,
	release: Arc<Notify>,
	reservations: Arc<Mutex<Option<ReservationCheck>>>,
	compactions: Arc<Mutex<Vec<Value>>>,
	compaction_status: Arc<AtomicUsize>,
	compaction_reservations: Arc<Mutex<Option<ReservationCheck>>>,
	force_memory_write: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ReservationCheck {
	pools: Vec<sqlx::PgPool>,
	dispatcher: sqlx::PgPool,
	grant: Uuid,
	admission: Uuid,
	purpose: &'static str,
}
impl ReservationCheck {
	async fn before_http(&self) {
		for pool in &self.pools {
			let count: i64 = sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust("COUNT(*)"))
					.from(Alias::new("generation_remote_usage"))
					.and_where(Expr::cust(
						"grant_id=$1 AND admission_id=$2 AND purpose=$3 AND state='RESERVED'",
					))
					.to_string(PostgresQueryBuilder),
			)
			.bind(self.grant)
			.bind(self.admission)
			.bind(self.purpose)
			.fetch_one(pool)
			.await
			.unwrap();
			assert_eq!(
				count, 1,
				"every allowance owner must have committed its reservation before provider HTTP"
			);
		}
		let count: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("generation_remote_dispatches"))
				.and_where(Expr::cust(
					"usage->>'grant_id'=$1 AND usage->>'purpose'=$2 AND state='DISPATCHED'",
				))
				.to_string(PostgresQueryBuilder),
		)
		.bind(self.grant.to_string())
		.bind(self.purpose)
		.fetch_one(&self.dispatcher)
		.await
		.unwrap();
		assert_eq!(
			count, 1,
			"dispatch admission must commit before provider HTTP"
		);
	}
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
	receiver_token: String,
	task: Uuid,
	grant: Uuid,
	admission: Uuid,
	requests: Arc<Mutex<Vec<Value>>>,
	servers: Vec<tokio::task::JoinHandle<()>>,
	au: String,
	bu: String,
	aschema: String,
	bschema: String,
	semantic: Option<SemanticFixture>,
	generation: Option<(Value, Value)>,
}
struct SemanticFixture {
	requests: Arc<Mutex<Vec<Value>>>,
	response: Arc<Mutex<Option<Value>>>,
	entry: Uuid,
	failing: Arc<AtomicBool>,
	reservations: Arc<Mutex<Option<ReservationCheck>>>,
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
		// One step can include several separately bounded peer/provider calls.
		// Allow their combined duration on loaded CI runners without changing
		// any production timeout, retry deadline or lease.
		tokio::time::timeout(std::time::Duration::from_secs(60), async {
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
	let semantic = request.uri().path().ends_with("/scoped/semantic/query");
	if !semantic && !request.uri().path().ends_with("/scoped/execution/commands") {
		return next.run(request).await;
	}
	let (parts, body) = request.into_parts();
	let bytes = to_bytes(body, 4 * 1024 * 1024).await.unwrap();
	let input: Value = serde_json::from_slice(&bytes).unwrap();
	let drop = {
		let mut wanted = state.lock().await;
		if wanted.as_deref().is_some_and(|value| {
			if semantic {
				value == "semantic.query"
			} else {
				input["operation"] == value
			}
		}) {
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
async fn scoped_pair(
	#[default(false)] semantic: bool,
	#[default(false)] generated: bool,
	#[default(false)] approval: bool,
	#[default(false)] compactor: bool,
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) -> Pair {
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
        let check=script.reservations.lock().await.clone();
        if let Some(check)=check {check.before_http().await;}
        let mut calls=script.requests.lock().await;
        calls.push(input);
        let count=calls.len(); drop(calls); script.entered.notify_one(); if script.hold.load(Ordering::Acquire) {script.release.notified().await;} let message=if count==1 && script.force_memory_write.load(Ordering::Acquire) {json!({"role":"assistant","content":null,"tool_calls":[{"id":"forbidden-write","type":"function","function":{"name":"memory_write","arguments":"{\"data\":{\"forbidden\":true}}"}}]})} else if count==1 {json!({"role":"assistant","content":null,"tool_calls":[{"id":"note","type":"function","function":{"name":"workspace_message","arguments":"{\"content\":\"Scoped remote progress\"}"}}]})} else {json!({"role":"assistant","content":"Scoped remote result"})};
        Json(json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    })).route("/systemone", post(|State(script):State<ModelScript>,Json(input):Json<Value>|async move {
		if let Some(check)=script.compaction_reservations.lock().await.clone() {check.before_http().await;}
		script.compactions.lock().await.push(input.clone());
		match script.compaction_status.load(Ordering::Acquire) {
			503 => (axum::http::StatusCode::SERVICE_UNAVAILABLE,"private compactor response").into_response(),
			401 => (axum::http::StatusCode::UNAUTHORIZED,"private compactor response").into_response(),
			1 => Json(json!({"answers":{}})).into_response(),
			_ => {
				let answers:serde_json::Map<_,_>=input["questions"].as_object().unwrap().keys().map(|name|(name.clone(),json!({"noul":0.0}))).collect();
				Json(json!({"answers":answers})).into_response()
			}
		}
	})).with_state(model_state.clone());
	let model_server = tokio::spawn(async move { axum::serve(model, model_app).await.unwrap() });
	let al = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let bl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	a.config.endpoint = format!("http://{}", al.local_addr().unwrap());
	b.config.endpoint = format!("http://{}", bl.local_addr().unwrap());
	let drop_reply = Arc::new(Mutex::new(None));
	let aa = source_router(&a, drop_reply.clone());
	let ba = api::router(b.clone());
	let (mut source_policy, token, mut task) = bootstrap(&a, &aa, &endpoint).await;
	let (receiver_policy, _, _) = bootstrap(&b, &ba, &endpoint).await;
	let compactor_definition = json!({"id":"remote-compactor","version":"1.0.0","kind":"compactor","name":{"en":"Approved remote compactor"},"description":{"en":"Local fixture"},"config":{"provider":"typesafe-system-one","endpoint":format!("{endpoint}/systemone"),"model":"fixture-jev","credential_env":"AIDASH_SECRET_TEST_PEER","max_request_bytes":400000,"max_questions":200,"max_response_bytes":16000}});
	let (status, body) = request(
		&ba,
		&b.config.api_token,
		"POST",
		"/api/registry",
		compactor_definition,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		request(
			&ba,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"remote-compactor","version":"1.0.0"},"expected_revision":0,"enabled":true})
		)
		.await
		.0,
		200
	);
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
	if semantic {
		let (_, home_reader) = request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"}),
		)
		.await;
		let (status,body) = request(&aa,&a.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":b.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":home_reader["credential"]["id"],"enabled":true,"expected_revision":0})).await;
		assert_eq!(status, 200, "{body}");
	}
	let aapp = aa.clone();
	let bapp = ba.clone();
	let aserver = tokio::spawn(async move { axum::serve(al, aapp).await.unwrap() });
	let bserver = tokio::spawn(async move { axum::serve(bl, bapp).await.unwrap() });
	let mut servers = vec![model_server, aserver, bserver];
	let semantic = if semantic {
		let workspace = a.store.task(task).await.unwrap().workspace_id;
		let (fixture, server) = semantic_fixture(
			&a,
			&aa,
			&token,
			workspace,
			&b.config.node_id,
			&test_environment.qdrant_url,
		)
		.await;
		servers.push(server);
		Some(fixture)
	} else {
		None
	};
	let generation = if generated {
		let (created, input, prepared) =
			prepare_generated_pair(&a, &b, &aa, &ba, &token, task, approval).await;
		task = created;
		Some((input, prepared))
	} else {
		None
	};
	let agent = generation.as_ref().map_or_else(
		|| json!({"id":"research","version":"1.0.0"}),
		|(_, prepared)| prepared["agent"].clone(),
	);
	let grant = Uuid::new_v4();
	let mut mode = if semantic.is_some() {
		json!({"mode":"required_home","embedding":{"id":"home-embedding","version":"1.0.0"}})
	} else {
		json!({"mode":"disabled"})
	};
	if compactor {
		mode["compactor"] = json!({"id":"remote-compactor","version":"1.0.0"});
	}
	let admission = if approval {
		Uuid::nil()
	} else {
		let (status, prepared) = request(
			&aa,
			&token,
			"POST",
			&format!("/api/tasks/{task}/remote-grants"),
			json!({"id":grant,"node_id":b.config.node_id,"agent":agent,"ttl_seconds":300,"semantic":mode}),
		)
		.await;
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
		serde_json::from_value(activated["admission_id"].clone()).unwrap()
	};
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
		receiver_token: credential["token"].as_str().unwrap().into(),
		servers,
		au,
		bu,
		aschema,
		bschema,
		semantic,
		generation,
	}
}

async fn semantic_fixture(
	a: &Federation,
	app: &Router,
	token: &str,
	workspace: Uuid,
	executor: &str,
	qdrant: &str,
) -> (SemanticFixture, tokio::task::JoinHandle<()>) {
	let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
	let failing = Arc::new(AtomicBool::new(false));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let reservations = Arc::new(Mutex::new(None::<ReservationCheck>));
	let response = Arc::new(Mutex::new(None::<Value>));
	type EmbeddingState = (
		Arc<Mutex<Vec<Value>>>,
		Arc<AtomicBool>,
		Arc<Mutex<Option<ReservationCheck>>>,
		Arc<Mutex<Option<Value>>>,
	);
	let embedding_app = Router::new().route("/v1/embeddings", post(|State((requests, failing, reservations, response)): State<EmbeddingState>, headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
		assert!(!headers.contains_key(axum::http::header::AUTHORIZATION), "an unsigned provider must not receive the subject or peer bearer");
        let check=reservations.lock().await.clone();
        if let Some(check)=check {check.before_http().await;}
		requests.lock().await.push(body.clone());
        if failing.load(Ordering::Acquire) {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"fixture outage"}))).into_response();
        }
		if let Some(value) = response.lock().await.clone() { return Json(value).into_response(); }
        Json(json!({"model":body["model"],"data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}})).into_response()
	})).with_state((requests.clone(), failing.clone(),reservations.clone(),response.clone()));
	let server = tokio::spawn(async move { axum::serve(listener, embedding_app).await.unwrap() });
	let embedding = json!({"provider":"openai","endpoint":endpoint,"credential_env":null,"model":"home-vector","model_version":"1","dimensions":3});
	let (status, body) = request(app, &a.config.api_token, "POST", "/api/registry", json!({"id":"home-embedding","version":"1.0.0","kind":"embedding","name":{"en":"Home embedding"},"description":{"en":"Deterministic semantic fixture"},"config":embedding})).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		&a.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"home-embedding","version":"1.0.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(app, &a.config.api_token, "POST", &format!("/api/workspaces/{workspace}/semantic/index"), json!({"expected_revision":0,"spec":{
		"embedding":embedding,"vector":{"provider":"qdrant","endpoint":qdrant,"credential_env":"AIDASH_SECRET_TEST_QDRANT"},
		"enabled":true,"auto_context":true,"max_sources":100,"max_results":10,"max_result_tokens":4096,"max_input_bytes":32768
	}})).await;
	assert_eq!(status, 200, "{body}");
	let mut first = Uuid::nil();
	for (key, owner, text) in [
		("shared", None, "Iridium archive marker: ochre falcon."),
		(
			"exact",
			Some(qualified_agent(executor, "research", "1.0.0")),
			"Exact executor marker: silver fern.",
		),
		(
			"other-node",
			Some(qualified_agent(&a.config.node_id, "research", "1.0.0")),
			"DO NOT DISCLOSE: same name at Home.",
		),
		(
			"other-version",
			Some(qualified_agent(executor, "research", "1.0.1")),
			"DO NOT DISCLOSE: other definition version.",
		),
	] {
		let (status, body) = request(app, token, "POST", &format!("/api/workspaces/{workspace}/semantic/entries"), json!({"key":key,"expected_revision":0,"source":{"kind":"memory","text":text},"agent":owner,"metadata":{}})).await;
		assert_eq!(status, 200, "{body}");
		if key == "shared" {
			first = serde_json::from_value(body["id"].clone()).unwrap();
		}
	}
	aidash::semantic::worker::sweep(&a.store).await.unwrap();
	(
		SemanticFixture {
			response,
			reservations,
			requests,
			entry: first,
			failing,
		},
		server,
	)
}

#[rstest::rstest]
#[tokio::test]
async fn remote_semantic_context_reaches_actual_model_with_home_scope_and_receipts(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.step().await;
	let run = p.run().await;
	assert_eq!(run.control, "ACTIVE", "{run:?}");
	let requests = p.requests.lock().await;
	assert_eq!(requests.len(), 1, "{run:?}");
	assert!(
		requests[0]["tools"]
			.as_array()
			.unwrap()
			.iter()
			.all(|tool| tool["function"]["name"] != "memory_write")
	);
	let context: Value =
		serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
	let semantic = &context["current"]["semantic_memory"];
	assert_eq!(semantic["home_node"], p.a.config.node_id, "{context}");
	assert_eq!(
		semantic["executor"],
		qualified_agent(&p.b.config.node_id, "research", "1.0.0")
	);
	assert_eq!(semantic["grant_id"], p.grant.to_string());
	let encoded = serde_json::to_string(semantic).unwrap();
	assert!(encoded.contains("ochre falcon"));
	assert!(encoded.contains("silver fern"));
	assert!(!encoded.contains("DO NOT DISCLOSE"));
	assert_eq!(semantic["result"]["matches"].as_array().unwrap().len(), 2);
	assert_eq!(semantic["sources"].as_array().unwrap().len(), 2);
	drop(requests);
	let calls = p.semantic.as_ref().unwrap().requests.lock().await;
	assert_eq!(calls.len(), 5, "four indexing calls and one search");
	assert!(
		!calls.last().unwrap()["input"]
			.as_str()
			.unwrap()
			.contains("ochre falcon"),
		"the query does not contain the retrieved text"
	);
	drop(calls);
	let saved: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("operation_id")).count())
			.from(Alias::new("semantic_remote_receipts"))
			.and_where(Expr::cust("run_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(p.admission)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(saved, 1);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_dependencies_hide_both_node_outputs_and_journals_after_source_change(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	for _ in 0..10 {
		if p.run().await.phase == "COMPLETED" {
			break;
		}
		p.step().await;
		assert_eq!(p.run().await.control, "ACTIVE", "{:?}", p.run().await);
	}
	assert_eq!(p.run().await.phase, "COMPLETED", "{:?}", p.run().await);
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let path = format!("/api/runs/{}", p.admission);
	let (status, body) = request(&p.ba, &p.receiver_token, "GET", &path, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["run"]["id"], p.admission.to_string());
	let home = format!("/api/workspaces/{workspace}");
	let (status, body) = request(&p.aa, &p.token, "GET", &home, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert!(body.to_string().contains("Scoped remote result"), "{body}");
	assert!(
		body.to_string().contains("Scoped remote progress"),
		"{body}"
	);
	let artifact = body["artifacts"][0]["id"].clone();
	let receipt_path = format!("/api/tasks/{}/remote-grants/{}/semantic", p.task, p.grant);
	let (status, receipt) = request(&p.aa, &p.token, "GET", &receipt_path, json!({})).await;
	assert_eq!(status, 200, "{receipt}");
	assert_eq!(receipt["sources"].as_array().unwrap().len(), 2);
	assert!(
		!receipt.to_string().contains("ochre falcon"),
		"provenance must not return source text"
	);
	let response =
		p.aa.clone()
			.oneshot(
				axum::http::Request::get(format!("/api/events/stream?workspace_id={workspace}"))
					.header("authorization", format!("Bearer {}", p.token))
					.body(Body::empty())
					.unwrap(),
			)
			.await
			.unwrap();
	assert_eq!(response.status(), 200);
	let mut stream = response.into_body().into_data_stream();
	tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	let source = p.semantic.as_ref().unwrap().entry;
	let (status, body) = request(
		&p.aa,
		&p.token,
		"DELETE",
		&format!("/api/workspaces/{workspace}/semantic/entries/{source}"),
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&p.ba, &p.receiver_token, "GET", &path, json!({})).await;
	assert_eq!(status, 403, "{body}");
	assert!(!body.to_string().contains("ochre falcon"));
	let (status, body) = request(&p.aa, &p.token, "GET", &home, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert!(!body.to_string().contains("Scoped remote result"), "{body}");
	assert!(
		!body.to_string().contains("Scoped remote progress"),
		"{body}"
	);
	assert_eq!(
		request(&p.aa, &p.token, "GET", &receipt_path, json!({}))
			.await
			.0,
		403
	);
	let (status,body)=request(&p.aa,&p.token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"derived-reingestion","expected_revision":0,"source":{"kind":"artifact","id":artifact},"metadata":{}})).await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/workspaces/{workspace}/messages"),
		json!({"content":"independent-stream-tail"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			let chunk = stream.next().await.unwrap().unwrap();
			let frame = String::from_utf8_lossy(&chunk);
			assert!(
				!frame.contains("Scoped remote result")
					&& !frame.contains("Scoped remote progress"),
				"{frame}"
			);
			if frame.contains("independent-stream-tail") {
				break;
			}
		}
	})
	.await
	.unwrap();
	drop(stream);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_journal_reads_require_the_current_viewer_at_home(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.step().await;
	let mut source_policy = p.source_policy.clone();
	let mut receiver_policy = p.receiver_policy.clone();
	source_policy["subjects"]["bob"] = json!({"kind":"user"});
	receiver_policy["subjects"]["bob"] = json!({"kind":"user"});
	for (node, app, bundle, revision) in [
		(&p.a, &p.aa, &source_policy, 2),
		(&p.b, &p.ba, &receiver_policy, 1),
	] {
		let (status, body) = request(
			app,
			&node.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":revision,"bundle":bundle}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let (_, viewer) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let token = viewer["token"].as_str().unwrap();
	let path = format!("/api/runs/{}", p.admission);
	assert_eq!(
		request(&p.ba, token, "GET", &path, json!({})).await.0,
		403,
		"producer authority must not stand in for an unmapped reader"
	);
	let (_, home_viewer) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let (status,body)=request(&p.aa,&p.a.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":p.b.config.node_id,"source_tenant":"acme","source_subject":"bob","credential_id":home_viewer["credential"]["id"],"enabled":true,"expected_revision":0})).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&p.ba, token, "GET", &path, json!({})).await;
	assert_eq!(status, 200, "{body}");
	source_policy["policies"].as_array_mut().unwrap().push(json!({"id":"reader-source-denial","effect":"deny","subjects":{"ids":["bob"]},"actions":["semantic.read"],"resources":{"kinds":["*"]}}));
	let (status, body) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":source_policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(request(&p.ba, token, "GET", &path, json!({})).await.0, 403);
	assert_eq!(
		request(&p.ba, &p.receiver_token, "GET", &path, json!({}))
			.await
			.0,
		200,
		"the producer still has authority; only the current reader was denied"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_empty_receipt_replay_rechecks_candidates_without_an_empty_embedding_charge(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let ids: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("semantic_entries"))
			.and_where(Expr::cust(
				"workspace_id=$1 AND (agent IS NULL OR agent=$2)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(qualified_agent(&p.b.config.node_id, "research", "1.0.0"))
	.fetch_all(&p.a.store.pool)
	.await
	.unwrap();
	assert_eq!(ids.len(), 2);
	for id in ids {
		let (status, body) = request(
			&p.aa,
			&p.token,
			"DELETE",
			&format!("/api/workspaces/{workspace}/semantic/entries/{id}"),
			json!({"expected_revision":1}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	p.step().await;
	*p.drop_reply.lock().await = Some("semantic.query".into());
	p.step().await;
	assert!(p.requests.lock().await.is_empty());
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"empty candidate retrieval does not call embedding"
	);
	let receipt: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("receipt"))
			.from(Alias::new("semantic_remote_operations"))
			.and_where(Expr::cust("grant_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(p.grant)
	.fetch_one(&p.a.store.pool)
	.await
	.unwrap();
	assert_eq!(receipt["sources"], json!([]));
	let (status,body)=request(&p.aa,&p.token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"new-current-source","expected_revision":0,"source":{"kind":"memory","text":"Newly admitted cobalt raven."},"metadata":{}})).await;
	assert_eq!(status, 200, "{body}");
	aidash::semantic::worker::sweep(&p.a.store).await.unwrap();
	p.step().await;
	let requests = p.requests.lock().await;
	assert_eq!(requests.len(), 1, "{:?}", p.run().await);
	let body: Value =
		serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
	assert!(
		body["current"]["semantic_memory"]
			.to_string()
			.contains("cobalt raven")
	);
	assert_eq!(
		body["current"]["semantic_memory"]["sources"]
			.as_array()
			.unwrap()
			.len(),
		1
	);
	drop(requests);
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		6,
		"one new index embedding and one fresh retrieval embedding"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn consumed_home_semantic_revision_blocks_next_remote_inference(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1, "{:?}", p.run().await);
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let source = p.semantic.as_ref().unwrap().entry;
	let (status, body) = request(
		&p.aa,
		&p.token,
		"DELETE",
		&format!("/api/workspaces/{workspace}/semantic/entries/{source}"),
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	p.step().await;
	assert_eq!(p.run().await.control, "PAUSED");
	assert_eq!(p.requests.lock().await.len(), 1);
	let control = format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant);
	let management = format!("/api/runs/{}/management", p.admission);
	let (status, body) = request(&p.ba, &p.receiver_token, "GET", &management, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["semantic_reason"], "invalidated");
	assert!(body.get("context").is_none() && body.get("pending").is_none());
	assert!(!body.to_string().contains("ochre falcon"));
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&control,
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	let (status, body) = request(
		&p.ba,
		&p.receiver_token,
		"POST",
		&management,
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["control"], "CANCELLED");
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&control,
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(!body.to_string().contains("ochre falcon"));
	let followup = format!("/api/tasks/{}/remote-grants/{}/follow-up", p.task, p.grant);
	let input = json!({"id":Uuid::new_v4(),"title":"New independent intent","description":"Use only current authorized material.","requirements":{}});
	let (status, body) = request(&p.aa, &p.token, "POST", &followup, input.clone()).await;
	assert_eq!(status, 200, "{body}");
	assert_ne!(body["id"], p.task.to_string());
	assert!(body["parent_id"].is_null());
	assert_eq!(body["description"], input["description"]);
	assert_eq!(
		body,
		request(&p.aa, &p.token, "POST", &followup, input).await.1
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn lost_semantic_reply_replays_receipt_without_a_second_embedding(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	*p.drop_reply.lock().await = Some("semantic.query".into());
	p.step().await;
	assert!(p.requests.lock().await.is_empty());
	let retry = p.run().await;
	assert_eq!(retry.pending["retry_count"], 1);
	let due: chrono::DateTime<chrono::Utc> =
		serde_json::from_value(retry.pending["retry_at"].clone()).unwrap();
	assert!(due > chrono::Utc::now());
	assert_eq!(p.run().await.control, "ACTIVE", "{:?}", p.run().await);
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1, "{:?}", p.run().await);
	assert_eq!(p.semantic.as_ref().unwrap().requests.lock().await.len(), 5);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn source_deleted_during_remote_model_request_rejects_the_response(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.model.hold.store(true, Ordering::Release);
	let worker = Harness {
		federation: p.b.clone(),
	};
	let running = tokio::spawn(async move { worker.worker_once().await });
	tokio::time::timeout(
		std::time::Duration::from_secs(20),
		p.model.entered.notified(),
	)
	.await
	.unwrap();
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let source = p.semantic.as_ref().unwrap().entry;
	let (status, body) = request(
		&p.aa,
		&p.token,
		"DELETE",
		&format!("/api/workspaces/{workspace}/semantic/entries/{source}"),
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	p.model.release.notify_waiters();
	running.await.unwrap().unwrap();
	assert_eq!(p.run().await.control, "PAUSED", "{:?}", p.run().await);
	assert!(
		!p.run()
			.await
			.context
			.to_string()
			.contains("Scoped remote progress")
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_retry_exhaustion_pauses_and_manual_resume_keeps_attempt_history(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.semantic
		.as_ref()
		.unwrap()
		.failing
		.store(true, Ordering::Release);
	for failure in 1..=6 {
		p.step().await;
		let run = p.run().await;
		assert!(p.requests.lock().await.is_empty());
		assert_eq!(
			run.control,
			if failure == 6 { "PAUSED" } else { "ACTIVE" },
			"{run:?}"
		);
		if failure < 6 {
			assert_eq!(run.pending["retry_count"], failure);
			let due: chrono::DateTime<chrono::Utc> =
				serde_json::from_value(run.pending["retry_at"].clone()).unwrap();
			assert!(due > chrono::Utc::now());
			// Advance only persisted fixture deadlines, after checking that the
			// production path saved a delay and retained its failure counter.
			sqlx::query(
				&Query::update()
					.table(Alias::new("runs"))
					.value(
						Alias::new("pending"),
						Expr::cust("JSONB_SET(pending, '{retry_at}', TO_JSONB(CLOCK_TIMESTAMP()))"),
					)
					.and_where(Expr::cust(
						"id=$1 AND SET_CONFIG('aidash.input_ledger_worker','true',true)='true'",
					))
					.to_string(PostgresQueryBuilder),
			)
			.bind(p.admission)
			.execute(&p.b.store.pool)
			.await
			.unwrap();
			sqlx::query(
				&Query::update()
					.table(Alias::new("semantic_remote_operations"))
					.value(Alias::new("next_attempt"), Expr::cust("CLOCK_TIMESTAMP()"))
					.and_where(Expr::cust("grant_id=$1 AND state='WAITING'"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(p.grant)
			.execute(&p.a.store.pool)
			.await
			.unwrap();
		}
	}
	let count = || {
		Query::select()
			.expr(Expr::col(Alias::new("id")).count())
			.from(Alias::new("semantic_remote_attempts"))
			.to_string(PostgresQueryBuilder)
	};
	let before: i64 = sqlx::query_scalar(&count())
		.fetch_one(&p.a.store.pool)
		.await
		.unwrap();
	assert_eq!(before, 6);
	p.semantic
		.as_ref()
		.unwrap()
		.failing
		.store(false, Ordering::Release);
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(p.run().await.pending.get("retry_count").is_none());
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1, "{:?}", p.run().await);
	let after: i64 = sqlx::query_scalar(&count())
		.fetch_one(&p.a.store.pool)
		.await
		.unwrap();
	assert_eq!(after, 7);
	let cycles: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("cycle")).sum())
			.from(Alias::new("semantic_remote_operations"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&p.a.store.pool)
	.await
	.unwrap();
	assert_eq!(cycles, 1);
	p.close().await;
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
async fn transaction_finalization_uses_the_actual_home_and_executor_admission(
	#[future(awt)] scoped_pair: Pair,
) {
	use aidash::transactions::{Manifest, coordinator, participant};
	let p = scoped_pair;
	for (local, app, remote) in [(&p.a, &p.aa, &p.b), (&p.b, &p.ba, &p.a)] {
		assert_eq!(
			request(
				app,
				&local.config.api_token,
				"POST",
				"/api/transactions/trust",
				json!({"node_id":remote.config.node_id,"enabled":true})
			)
			.await
			.0,
			200
		);
	}
	for _ in 0..4 {
		if p.a.store.task(p.task).await.unwrap().status == "RUNNING" {
			break;
		}
		p.step().await;
	}
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(task.status, "RUNNING");
	let run = p.run().await;
	sqlx::query(&Query::update().table(Alias::new("runs"))
		.value(Alias::new("phase"),"TOOL_CALL").value(Alias::new("pending"),Expr::cust("$2"))
		.and_where(Expr::cust("id=$1")).to_string(PostgresQueryBuilder))
		.bind(run.id).bind(json!({"response":{"text":"Atomic remote answer","tool_calls":[],"input_tokens":0,"output_tokens":0},"cursor":0}))
		.execute(&p.b.store.pool).await.unwrap();
	let mut manifest:Manifest=serde_json::from_value(json!({"id":Uuid::new_v4(),"coordinator":p.a.config.node_id,"isolation":"serializable","deadline":chrono::Utc::now()+chrono::Duration::minutes(5),
		"participants":[{"node_id":p.a.config.node_id,"mutations":[{"kind":"complete_task","task_id":task.id,"expected_revision":task.revision,"artifact":{"kind":"text","name":"Answer","content":"Atomic remote answer"}}]},
		{"node_id":p.b.config.node_id,"mutations":[{"kind":"finish_run","run_id":run.id,"task_id":task.id,"expected_revision":run.revision}]}]})).unwrap();
	manifest
		.participants
		.sort_by(|a, b| a.node_id.cmp(&b.node_id));
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 202, "{body}");
	for _ in 0..2 {
		coordinator::advance(&p.a, manifest.id).await.unwrap();
	}
	let (status, body) = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/transactions/{}", manifest.id),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 200,
		"current authorized controls remain available during reservation: {body}"
	);
	for _ in 0..20 {
		if coordinator::advance(&p.a, manifest.id)
			.await
			.unwrap()
			.complete
		{
			break;
		}
	}
	let result = coordinator::status(&p.a, manifest.id).await.unwrap();
	assert!(result.complete, "{result:?}");
	assert_eq!(result.decision.as_deref(), Some("COMMIT"), "{result:?}");
	for local in [&p.a, &p.b] {
		participant::finish(local, &p.a.config.node_id, &manifest)
			.await
			.unwrap();
	}
	assert_eq!(p.a.store.task(task.id).await.unwrap().status, "COMPLETED");
	assert_eq!(p.run().await.phase, "COMPLETED");
	let artifacts =
		p.a.store
			.snapshot(task.workspace_id)
			.await
			.unwrap()
			.artifacts;
	assert_eq!(artifacts.len(), 1);
	assert_eq!(artifacts[0].created_by, task.owner.unwrap());
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

async fn prepare_generated_pair(
	a: &Federation,
	b: &Federation,
	aa: &Router,
	ba: &Router,
	token: &str,
	task: Uuid,
	approval: bool,
) -> (Uuid, Value, Value) {
	let workspace = a.store.task(task).await.unwrap().workspace_id;
	let (_, mut parent_template) = request(
		aa,
		&a.config.api_token,
		"GET",
		"/api/registry/research/1.0.0",
		Value::Null,
	)
	.await;
	let (_, child_template) = request(
		ba,
		&b.config.api_token,
		"GET",
		"/api/registry/research/1.0.0",
		Value::Null,
	)
	.await;
	parent_template["capabilities"] = json!(["unique-parent-specialist"]);
	let (_, embedding) = request(
		aa,
		&a.config.api_token,
		"GET",
		"/api/registry/home-embedding/1.0.0",
		Value::Null,
	)
	.await;
	let model_ref = child_template["config"]["model"].clone();
	let (_, model) = request(
		ba,
		&b.config.api_token,
		"GET",
		&format!(
			"/api/registry/{}/{}",
			model_ref["id"].as_str().unwrap(),
			model_ref["version"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	let descriptor = |node: &str, entry: &Value| json!({"node_id":node,"entry":{"id":entry["id"],"version":entry["version"]},"digest":aidash::registry::digest(entry),"configuration_digest":aidash::registry::digest(&entry["config"])});
	let embedding_descriptor = descriptor(&a.config.node_id, &embedding);
	let model_descriptor = descriptor(&b.config.node_id, &model);
	let (_, compactor) = request(
		ba,
		&b.config.api_token,
		"GET",
		"/api/registry/remote-compactor/1.0.0",
		Value::Null,
	)
	.await;
	let compactor_descriptor = descriptor(&b.config.node_id, &compactor);
	let common = json!({"enabled":true,"permissions":{"roles":[],"groups":[],"attributes":{}},"approval_required":false,"limits":{"max_agents":4,"max_concurrent":4,"max_depth":4,"token_budget":4000000,"tokens_per_agent":800000,"lifetime_seconds":3600}});
	let mut parent_spec = common.clone();
	parent_spec["template"] = parent_template;
	parent_spec["embedding"] = json!({"provider":{"id":"home-embedding","version":"1.0.0"},"calls_per_agent":10,"call_budget":40});
	parent_spec["remote"] = json!({"inference":[model_descriptor],"compaction":{"provider":compactor_descriptor,"calls_per_agent":2,"call_budget":8}});
	let (status, body) = request(
		aa,
		&a.config.api_token,
		"POST",
		"/api/generation/acme/policies/remote-parent",
		json!({"expected_revision":0,"spec":parent_spec}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status,parent)=request(aa,token,"POST",&format!("/api/workspaces/{workspace}/tasks"),json!({"title":"Generated origin","description":"Delegate scoped memory research","requirements":{"capability":"unique-parent-specialist"}})).await;
	assert_eq!(status, 200, "{parent}");
	let parent_id: Uuid = serde_json::from_value(parent["id"].clone()).unwrap();
	let (status, parent) = request(
		aa,
		token,
		"POST",
		&format!("/api/generation/acme/tasks/{parent_id}/assign"),
		json!({"policy_id":"remote-parent","reason":"ancestor restriction fixture"}),
	)
	.await;
	assert_eq!(status, 200, "{parent}");
	assert_eq!(parent["kind"], "generated");
	aidash::generation::provision::reconcile(a).await.unwrap();
	let parent_run: Uuid = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::cust("task_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(parent_id)
	.fetch_one(&a.store.pool)
	.await
	.unwrap();
	let parent_run = a.store.run(parent_run).await.unwrap();
	// Seed the trusted origin to isolate the two-owner allowance assertions.
	// The cluster driver creates this child through the actual task_create tool.
	let chain = vec![
		"alice".to_owned(),
		qualified_agent(
			&a.config.node_id,
			&parent_run.agent_id,
			&parent_run.agent_version,
		),
	];
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_task_origins"))
			.columns(
				[
					"task_id",
					"source_run_id",
					"tenant",
					"root_subject",
					"subject_chain",
				]
				.map(Alias::new),
			)
			.values_panic(["$1", "$2", "'acme'", "'alice'", "$3"].map(Expr::cust))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task)
	.bind(parent_run.id)
	.bind(chain)
	.execute(&a.store.pool)
	.await
	.unwrap();
	let mut child_spec = common;
	child_spec["approval_required"] = json!(approval);
	child_spec["template"] = child_template;
	child_spec["compaction"] = json!({"provider":{"id":"remote-compactor","version":"1.0.0"},"calls_per_agent":2,"call_budget":8});
	child_spec["remote"] = json!({"embedding":{"provider":embedding_descriptor,"calls_per_agent":10,"call_budget":40}});
	let (status, body) = request(
		ba,
		&b.config.api_token,
		"POST",
		"/api/generation/acme/policies/remote-child",
		json!({"expected_revision":0,"spec":child_spec}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let input = json!({"id":Uuid::new_v4(),"node_id":b.config.node_id,"policy_id":"remote-child","policy_revision":1,"ttl_seconds":600,"reason":"foreign task specialist"});
	let route = format!("/api/tasks/{task}/remote-generation");
	let (status, prepared) = request(aa, token, "POST", &route, input.clone()).await;
	assert_eq!(status, 200, "{prepared}");
	assert_eq!(prepared["prepared"], !approval);
	let (status, replayed) = request(aa, token, "POST", &route, input.clone()).await;
	assert_eq!(status, 200, "{replayed}");
	assert_eq!(replayed, prepared);
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("tasks"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(task)
	.fetch_one(&b.store.pool)
	.await
	.unwrap();
	assert_eq!(
		count, 0,
		"foreign preparation cannot create a fake local task"
	);
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("runs"))
			.and_where(Expr::cust("home_node=$1 AND task_id=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&a.config.node_id)
	.bind(task)
	.fetch_one(&b.store.pool)
	.await
	.unwrap();
	assert_eq!(count, 0, "preparation cannot activate before the grant");
	(task, input, prepared)
}

#[rstest::rstest]
#[tokio::test]
async fn foreign_generation_waits_for_approval_and_replays_one_exact_definition(
	#[with(true, true, true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let (input, pending) = p.generation.as_ref().unwrap();
	assert_eq!(pending["status"], "PENDING_APPROVAL");
	let route = format!("/api/tasks/{}/remote-generation", p.task);
	let (first, second) = tokio::join!(
		request(&p.aa, &p.token, "POST", &route, input.clone()),
		request(&p.aa, &p.token, "POST", &route, input.clone()),
	);
	for (status, body) in [first, second] {
		assert_eq!(status, 200, "{body}");
		assert_eq!(body, *pending);
	}
	let grant = json!({"id":p.grant,"node_id":p.b.config.node_id,"agent":pending["agent"],"ttl_seconds":300,"semantic":{"mode":"required_home","embedding":{"id":"home-embedding","version":"1.0.0"}}});
	let grant_route = format!("/api/tasks/{}/remote-grants", p.task);
	assert_ne!(
		request(&p.aa, &p.token, "POST", &grant_route, grant.clone())
			.await
			.0,
		200
	);
	let control = format!(
		"/api/generation/acme/requests/{}/control",
		pending["request_id"].as_str().unwrap()
	);
	let decision = json!({"action":"approve","reason":"Approve the exact foreign preparation"});
	for _ in 0..2 {
		let (status, body) = request(
			&p.ba,
			&p.b.config.api_token,
			"POST",
			&control,
			decision.clone(),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(body["status"], "QUEUED");
	}
	let (first, second) = tokio::join!(
		request(&p.aa, &p.token, "POST", &route, input.clone()),
		request(&p.aa, &p.token, "POST", &route, input.clone()),
	);
	for (status, body) in [first, second] {
		assert_eq!(status, 200, "{body}");
		assert_eq!(body["prepared"], true);
		assert_eq!(body["agent"], pending["agent"]);
		assert_eq!(body["request_id"], pending["request_id"]);
	}
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("runs"))
			.and_where(Expr::cust("home_node=$1 AND task_id=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&p.a.config.node_id)
	.bind(p.task)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(
		count, 0,
		"approval publishes the definition but cannot activate a Run"
	);
	let (status, body) = request(&p.aa, &p.token, "POST", &grant_route, grant).await;
	assert_eq!(status, 200, "{body}");
	let (status, activated) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{activated}");
	let (status, replayed) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{replayed}");
	assert_eq!(replayed["admission_id"], activated["admission_id"]);
	let rows: (i64, i64) = sqlx::query_as(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.expr(Expr::cust("COUNT(DISTINCT agent_id)"))
			.from(Alias::new("generation_requests"))
			.and_where(Expr::cust(
				"home_node=$1 AND task_id=$2 AND status='ACTIVE'",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&p.a.config.node_id)
	.bind(p.task)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(rows, (1, 1));
	p.close().await;
}

#[rstest::rstest]
#[case("deny", "DENIED")]
#[case("cancel", "STOPPED")]
#[case("expire", "EXPIRED")]
#[tokio::test]
async fn foreign_preparation_termination_releases_unused_allocations_once(
	#[with(true, true, true)]
	#[future(awt)]
	scoped_pair: Pair,
	#[case] action: &str,
	#[case] expected: &str,
) {
	let p = scoped_pair;
	let (_, pending) = p.generation.as_ref().unwrap();
	let job: Uuid = serde_json::from_value(pending["request_id"].clone()).unwrap();
	let route = format!("/api/generation/acme/requests/{job}/control");
	for _ in 0..2 {
		match action {
			"deny" => {
				let (status, body) = request(
					&p.ba,
					&p.b.config.api_token,
					"POST",
					&route,
					json!({"action":"deny","reason":"Decline foreign preparation"}),
				)
				.await;
				assert_eq!(status, 200, "{body}");
			}
			"cancel" => {
				let route = format!(
					"/api/tasks/{}/remote-generation/{}/cancel",
					p.task,
					pending["intent_id"].as_str().unwrap()
				);
				let (status, body) = request(&p.aa, &p.token, "POST", &route, json!({})).await;
				assert_eq!(status, 200, "{body}");
			}
			"expire" => {
				sqlx::query(
					&Query::update()
						.table(Alias::new("generation_requests"))
						.value(
							Alias::new("expires_at"),
							Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
						)
						.and_where(Expr::cust("id=$1"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(job)
				.execute(&p.b.store.pool)
				.await
				.unwrap();
				aidash::generation::provision::reconcile(&p.b)
					.await
					.unwrap();
			}
			_ => unreachable!(),
		}
	}
	let state: (String, bool, Option<Uuid>) = sqlx::query_as(
		&Query::select()
			.columns(["status", "quota_released", "admission_id"].map(Alias::new))
			.from(Alias::new("generation_requests"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(job)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(state, (expected.into(), true, None));
	let budgets: (i64, i64, i64) = sqlx::query_as(
		&Query::select()
			.columns(
				[
					"allocated_tokens",
					"allocated_embedding_calls",
					"allocated_compaction_calls",
				]
				.map(Alias::new),
			)
			.from(Alias::new("generation_policies"))
			.and_where(Expr::cust("tenant='acme' AND id='remote-child'"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(
		budgets,
		(0, 0, 0),
		"only unused allocations are released, exactly once"
	);
	let (status, _) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		&route,
		json!({"action":"approve","reason":"Late approval must not resurrect terminated work"}),
	)
	.await;
	assert_eq!(status, 409);
	assert_eq!(p.requests.lock().await.len(), 0);
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"preparation makes no query embedding call"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn generated_foreign_executor_and_home_ancestor_share_durable_provider_allowances(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let pair = scoped_pair;
	let expectation = ReservationCheck {
		pools: vec![pair.a.store.pool.clone(), pair.b.store.pool.clone()],
		dispatcher: pair.a.store.pool.clone(),
		grant: pair.grant,
		admission: pair.admission,
		purpose: "embedding",
	};
	*pair.semantic.as_ref().unwrap().reservations.lock().await = Some(expectation.clone());
	*pair.model.reservations.lock().await = Some(ReservationCheck {
		dispatcher: pair.b.store.pool.clone(),
		purpose: "inference",
		..expectation
	});

	for _ in 0..4 {
		pair.step().await;
		if !pair.requests.lock().await.is_empty() || pair.run().await.control == "PAUSED" {
			break;
		}
	}
	let requests = pair.requests.lock().await;
	assert_eq!(requests.len(), 1, "{:?}", pair.run().await);
	let input: Value =
		serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
	assert!(
		input["current"]["semantic_memory"]
			.to_string()
			.contains("ochre falcon")
	);
	drop(requests);

	for (dispatcher, owner) in [(&pair.a, &pair.b), (&pair.b, &pair.a)] {
		let records: Vec<(Value, Value)> = sqlx::query_as(
			&Query::select()
				.columns(["usage", "finalization"].map(Alias::new))
				.from(Alias::new("generation_remote_dispatches"))
				.and_where(Expr::cust("state='SETTLED'"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&dispatcher.store.pool)
		.await
		.unwrap();
		assert_eq!(records.len(), 1);
		for (usage, finalization) in records {
			let client = reqwest::Client::new();
			for _ in 0..3 {
				let response = client
					.post(format!(
						"{}/federation/v0.1/scoped/usage/finalize",
						owner.config.endpoint
					))
					.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
					.header("x-aidash-node", &dispatcher.config.node_id)
					.header("x-aidash-protocol", "0.1")
					.json(&json!({"usage":usage,"result":finalization}))
					.send()
					.await
					.unwrap();
				assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
			}
		}
	}
	for node in [&pair.a, &pair.b] {
		let usage: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
			&Query::select()
				.columns(["purpose", "state", "reserved_tokens", "reported_tokens"].map(Alias::new))
				.from(Alias::new("generation_remote_usage"))
				.and_where(Expr::cust("grant_id=$1 AND admission_id=$2"))
				.order_by(Alias::new("purpose"), sea_orm::sea_query::Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.bind(pair.grant)
		.bind(pair.admission)
		.fetch_all(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(
			usage.len(),
			2,
			"both embedding and inference must debit {}: {usage:?}",
			node.config.node_id
		);
		assert_eq!(usage[0].0, "embedding");
		assert_eq!(usage[0].1, "SETTLED");
		assert_eq!(usage[0].3, Some(1));
		assert_eq!(usage[1].0, "inference");
		assert_eq!(usage[1].1, "SETTLED");
		assert_eq!(usage[1].3, Some(2));
		let budget: (i64, i64) = sqlx::query_as(
			&Query::select()
				.columns(["used_tokens", "embedding_calls"].map(Alias::new))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(
			budget,
			(3, 1),
			"exact one-call settlement at {}",
			node.config.node_id
		);
		let (app, token, route) = if node.config.node_id == pair.a.config.node_id {
			(
				&pair.aa,
				&pair.token,
				format!(
					"/api/tasks/{}/remote-grants/{}/semantic",
					pair.task, pair.grant
				),
			)
		} else {
			(
				&pair.ba,
				&pair.receiver_token,
				format!("/api/runs/{}/semantic", pair.admission),
			)
		};
		let (status, view) = request(app, token, "GET", &route, Value::Null).await;
		assert_eq!(status, 200, "{view}");
		assert_eq!(view["allowance_node"], node.config.node_id);
		assert_eq!(view["allowances"].as_array().unwrap().len(), 1);
		assert_eq!(view["allowances"][0]["used_tokens"], 3);
		assert_eq!(view["allowances"][0]["embedding_calls"], 1);
		let (status, usage) = request(
			app,
			token,
			"GET",
			&format!(
				"/api/generation/acme/requests/{}/usage",
				view["allowances"][0]["request_id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{usage}");
		assert_eq!(usage["inference_attempts"], 1);
	}
	pair.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn operator_content_views_cannot_bypass_both_node_subject_authority(
	#[future(awt)]
	#[with(true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	for _ in 0..8 {
		p.step().await;
		if p.run().await.phase == "COMPLETED" {
			break;
		}
	}
	assert_eq!(p.run().await.phase, "COMPLETED");
	let task = p.a.store.task(p.task).await.unwrap();
	let (status, viewer) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{viewer}");
	assert_eq!(viewer["run"]["id"], p.admission.to_string());
	assert!(
		p.model.requests.lock().await[0]
			.to_string()
			.contains("ochre falcon")
	);
	for (f, app) in [(&p.a, &p.aa), (&p.b, &p.ba)] {
		for path in [
			"/api/state".to_owned(),
			"/api/events".into(),
			"/api/tasks".into(),
		] {
			let (status, body) = request(app, &f.config.api_token, "GET", &path, Value::Null).await;
			assert_eq!(status, 200, "{path}: {body}");
			for hidden in [
				"ochre falcon",
				"Scoped remote result",
				"Scoped remote progress",
			] {
				assert!(
					!body.to_string().contains(hidden),
					"operator path {} {path} leaked {hidden}: {:?}",
					f.config.node_id,
					body.as_object().map(|fields| fields
						.iter()
						.filter(|(_, value)| value.to_string().contains(hidden))
						.collect::<Vec<_>>())
				);
			}
		}
		// The notification-backed stream must enforce the same boundary at
		// delivery, including receiver Run events without a Workspace FK.
		let (status, body) = request(
			app,
			&f.config.api_token,
			"POST",
			"/api/workspaces",
			json!({"title":"operator-stream-tail","goal":"Independent stream marker"}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		let response = app
			.clone()
			.oneshot(
				axum::http::Request::get("/api/events/stream")
					.header("authorization", format!("Bearer {}", f.config.api_token))
					.body(Body::empty())
					.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(response.status(), 200);
		let mut stream = response.into_body().into_data_stream();
		tokio::time::timeout(std::time::Duration::from_secs(20), async {
			loop {
				let chunk = stream.next().await.unwrap().unwrap();
				let frame = String::from_utf8_lossy(&chunk);
				for hidden in [
					"ochre falcon",
					"Scoped remote result",
					"Scoped remote progress",
				] {
					assert!(
						!frame.contains(hidden),
						"operator stream leaked {hidden}: {frame}"
					);
				}
				if frame.contains("operator-stream-tail") {
					break;
				}
			}
		})
		.await
		.unwrap();
		drop(stream);
	}
	for path in [
		format!("/api/workspaces/{}", task.workspace_id),
		format!("/api/workspaces/{}/message-history", task.workspace_id),
	] {
		assert_eq!(
			request(&p.aa, &p.a.config.api_token, "GET", &path, Value::Null)
				.await
				.0,
			403
		);
	}
	assert_eq!(
		request(
			&p.ba,
			&p.b.config.api_token,
			"GET",
			&format!("/api/runs/{}", p.admission),
			Value::Null
		)
		.await
		.0,
		403
	);
	let (status, control) = request(
		&p.ba,
		&p.b.config.api_token,
		"GET",
		&format!("/api/runs/{}/management", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{control}");
	assert!(control.get("context").is_none());
	assert!(control.get("pending").is_none());
	assert!(control.get("error").is_none());
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	let artifact = snapshot.artifacts.first().unwrap();
	let (status,_)=request(&p.aa,&p.a.config.api_token,"POST",&format!("/api/workspaces/{}/semantic/entries",task.workspace_id),json!({"key":"operator-bypass","expected_revision":0,"source":{"kind":"artifact","id":artifact.id},"metadata":{}})).await;
	assert_eq!(status, 403);
	let (status, independent) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		"/api/workspaces",
		json!({"title":"Independent operator work","goal":"Unrelated legacy access"}),
	)
	.await;
	assert_eq!(status, 200, "{independent}");
	assert_eq!(
		request(
			&p.aa,
			&p.a.config.api_token,
			"GET",
			&format!("/api/workspaces/{}", independent["id"].as_str().unwrap()),
			Value::Null
		)
		.await
		.0,
		200
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_cross_node_producer_cycle_terminates_and_still_requires_receiver_authority(
	#[future(awt)]
	#[with(true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	for _ in 0..8 {
		p.step().await;
		if p.run().await.phase == "COMPLETED" {
			break;
		}
	}
	assert_eq!(p.run().await.phase, "COMPLETED");
	let task = p.a.store.task(p.task).await.unwrap();
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	let artifact = snapshot.artifacts.first().unwrap();
	// Reproduce a persisted observation cycle: A's grant reads an A Artifact
	// produced by that grant's B Run. Such provenance must neither recurse over
	// HTTP nor suppress the receiver's current-reader check.
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_remote_grant_reads"))
			.columns(["grant_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new))
			.values_panic(["$1", "$2", "'artifact'", "$3"].map(Expr::cust))
			.to_string(PostgresQueryBuilder),
	)
	.bind(p.grant)
	.bind(task.workspace_id)
	.bind(artifact.id)
	.execute(&p.a.store.pool)
	.await
	.unwrap();
	let path = format!("/api/tasks/{}/remote-grants/{}/semantic", p.task, p.grant);
	let (status, body) = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		request(&p.aa, &p.token, "GET", &path, Value::Null),
	)
	.await
	.expect("cross-node cycle must terminate");
	assert_eq!(status, 200, "{body}");
	let mut denied = p.receiver_policy.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-cycle-read","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["run"]}}));
	assert_eq!(
		request(
			&p.ba,
			&p.b.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":denied})
		)
		.await
		.0,
		200
	);
	let (status, body) = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		request(&p.aa, &p.token, "GET", &path, Value::Null),
	)
	.await
	.expect("denied cross-node cycle must also terminate");
	assert_eq!(status, 403, "{body}");
	let (status, body) = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/workspaces/{}", task.workspace_id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(body["artifacts"].as_array().unwrap().is_empty());
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn generated_home_allowance_failure_releases_only_predispatch_receiver_reservations(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let pair = scoped_pair;
	sqlx::query(
		&Query::update()
			.table(Alias::new("generation_budgets"))
			.value(Alias::new("embedding_call_limit"), 0)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&pair.a.store.pool)
	.await
	.unwrap();
	for _ in 0..4 {
		pair.step().await;
		if pair.run().await.control == "PAUSED" {
			break;
		}
	}
	let run = pair.run().await;
	assert_eq!(run.control, "PAUSED", "{run:?}");
	assert_eq!(run.pending["semantic_reason"], "allowance");
	assert!(pair.requests.lock().await.is_empty());
	assert_eq!(
		pair.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"only pre-existing index embeddings are permitted"
	);
	let debit: (i64, i64) = sqlx::query_as(
		&Query::select()
			.columns(["used_tokens", "embedding_calls"].map(Alias::new))
			.from(Alias::new("generation_budgets"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&pair.b.store.pool)
	.await
	.unwrap();
	assert_eq!(debit, (0, 0));
	let state: String = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("generation_remote_usage"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&pair.b.store.pool)
	.await
	.unwrap();
	assert_eq!(state, "RELEASED");
	pair.close().await;
}

async fn seed_remote_history(p: &Pair) {
	let mut history = vec![
		json!({"kind":"tool","call":{"id":"first","name":"read","arguments":{}},"result":"keep first"}),
		json!({"kind":"tool","call":{"id":"obsolete","name":"read","arguments":{}},"result":"old".repeat(50000)}),
	];
	for i in 0..6 {
		history.push(json!({"kind":"tool","call":{"id":format!("recent-{i}"),"name":"read","arguments":{}},"result":"recent"}));
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("phase"), "THINKING")
			.value(Alias::new("context"), Expr::cust("$2"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(p.admission)
	.bind(json!({"history":history,"summary":"","usage":{},"compactions":0}))
	.execute(&p.b.store.pool)
	.await
	.unwrap();
}

#[rstest::rstest]
#[case("approved", 0, None)]
#[case("unnecessary", 0, None)]
#[case("ancestor_limit", 0, Some("allowance"))]
#[case("catalog_revoked", 0, Some("authority"))]
#[case("outage", 503, Some("unavailable"))]
#[case("bad_answers", 1, Some("provider_contract"))]
#[case("credential_rejected", 401, Some("configuration"))]
#[tokio::test]
async fn remote_compaction_uses_exact_approval_and_origin_owned_allowances(
	#[future(awt)]
	#[with(true, true, false, true)]
	scoped_pair: Pair,
	#[case] scenario: &str,
	#[case] status: usize,
	#[case] reason: Option<&str>,
) {
	let p = scoped_pair;
	if scenario != "unnecessary" {
		seed_remote_history(&p).await;
	}
	p.model.compaction_status.store(status, Ordering::Release);
	*p.model.compaction_reservations.lock().await = Some(ReservationCheck {
		pools: vec![p.a.store.pool.clone(), p.b.store.pool.clone()],
		dispatcher: p.b.store.pool.clone(),
		grant: p.grant,
		admission: p.admission,
		purpose: "compaction",
	});
	if scenario == "ancestor_limit" {
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_budgets"))
				.value(Alias::new("compaction_call_limit"), 0)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&p.a.store.pool)
		.await
		.unwrap();
	}
	if scenario == "catalog_revoked" {
		assert_eq!(request(&p.ba,&p.b.config.api_token,"POST","/api/authorization/acme/catalog",json!({"entry":{"id":"remote-compactor","version":"1.0.0"},"expected_revision":1,"enabled":false})).await.0,200);
	}
	for _ in 0..4 {
		p.step().await;
		let run = p.run().await;
		if !p.requests.lock().await.is_empty()
			|| run.control == "PAUSED"
			|| run.pending.get("retry_at").is_some()
		{
			break;
		}
	}
	let run = p.run().await;
	if let Some(reason) = reason {
		assert_eq!(run.pending["semantic_reason"], reason, "{run:?}");
		assert_eq!(
			run.control,
			if scenario == "outage" {
				"ACTIVE"
			} else {
				"PAUSED"
			}
		);
		assert_ne!(run.phase, "FAILED");
		assert!(p.requests.lock().await.is_empty());
		assert!(
			!run.error
				.as_deref()
				.unwrap_or_default()
				.contains("private compactor response")
		);
	} else {
		assert_eq!(p.requests.lock().await.len(), 1, "{run:?}");
	}
	let count = usize::from(matches!(
		scenario,
		"approved" | "outage" | "bad_answers" | "credential_rejected"
	));
	let calls = p.model.compactions.lock().await;
	assert_eq!(calls.len(), count);
	if let Some(call) = calls.first() {
		assert_eq!(call["model"], "fixture-jev");
		assert!(
			call.to_string().contains("ochre falcon"),
			"approved context must reach the exact compactor"
		);
	}
	drop(calls);
	for node in [&p.a, &p.b] {
		let (used, limit): (i64, i64) = sqlx::query_as(
			&Query::select()
				.columns(["compaction_calls", "compaction_call_limit"].map(Alias::new))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(
			used, count as i64,
			"{}: {used}/{limit}",
			node.config.node_id
		);
	}
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_compaction_without_explicit_recipient_pauses_without_environment_fallback(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	seed_remote_history(&p).await;
	p.step().await;
	let run = p.run().await;
	assert_eq!(run.control, "PAUSED", "{run:?}");
	assert_eq!(run.pending["semantic_reason"], "context_budget");
	assert!(p.model.compactions.lock().await.is_empty());
	assert!(p.requests.lock().await.is_empty());
	p.close().await;
}

struct ScopedWorkerProcess(std::process::Child);
impl ScopedWorkerProcess {
	fn start(p: &Pair) -> Self {
		let mut database = reqwest::Url::parse(&p.bu).unwrap();
		database
			.query_pairs_mut()
			.append_pair("options", &format!("-c search_path={}", p.bschema));
		Self(
			std::process::Command::new(env!("CARGO_BIN_EXE_aidash"))
				.arg("worker")
				.env("DATABASE_URL", database.as_str())
				.env("NATS_URL", &p.b.config.nats_url)
				.env("AIDASH_NODE_ID", &p.b.config.node_id)
				.env("AIDASH_ENDPOINT", &p.b.config.endpoint)
				.env("AIDASH_API_TOKEN", &p.b.config.api_token)
				.env("RUST_LOG", "aidash=info")
				.stdout(std::process::Stdio::inherit())
				.stderr(std::process::Stdio::inherit())
				.spawn()
				.unwrap(),
		)
	}
}
impl Drop for ScopedWorkerProcess {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

#[rstest::rstest]
#[tokio::test]
async fn process_sigkill_preserves_remote_receipt_and_uncertain_origin_charges(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.model.hold.store(true, Ordering::Release);
	let mut worker = ScopedWorkerProcess::start(&p);
	let arrived = tokio::time::timeout(
		std::time::Duration::from_secs(30),
		p.model.entered.notified(),
	)
	.await;
	assert!(
		arrived.is_ok(),
		"worker={:?}, run={:?}",
		worker.0.try_wait(),
		p.run().await
	);
	drop(worker); // Actual SIGKILL. No worker cleanup or settlement runs.
	let old_attempt: Uuid = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("attempt_id"))
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust(
				"state='DISPATCHED' AND usage->>'purpose'='inference'",
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	let before = p.run().await;
	let before_receipts: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("semantic_remote_receipts"))
			.and_where(Expr::cust("run_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(p.admission)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(
		before_receipts, 1,
		"receipt must have survived before inference HTTP"
	);
	for node in [&p.a, &p.b] {
		let state: String = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("state"))
				.from(Alias::new("generation_remote_usage"))
				.and_where(Expr::cust("attempt_id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(old_attempt)
		.fetch_one(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(state, "RESERVED");
	}
	p.model.hold.store(false, Ordering::Release);
	p.model.release.notify_one();
	// Advance only the dead process's lease, retaining the original durable
	// attempt and fencing. Recovery otherwise waits the production lease delay.
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(
				Alias::new("lease_until"),
				Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
			)
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(p.admission)
	.execute(&p.b.store.pool)
	.await
	.unwrap();
	let worker = ScopedWorkerProcess::start(&p);
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		loop {
			let run = p.run().await;
			if run.phase == "COMPLETED" {
				break;
			}
			assert_ne!(run.control, "PAUSED", "{run:?}");
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	})
	.await
	.expect("same remote Run must recover under its original admission");
	drop(worker);
	let after = p.run().await;
	assert_eq!(after.id, before.id);
	assert_eq!(after.task_id, before.task_id);
	assert_eq!(after.agent_id, before.agent_id);
	assert_eq!(p.requests.lock().await.len(), 2);
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		5,
		"persisted receipt replay must not embed the same boundary again"
	);
	for node in [&p.a, &p.b] {
		let usages: Vec<(Uuid, String, i64, Option<i64>)> = sqlx::query_as(
			&Query::select()
				.columns(
					["attempt_id", "state", "reserved_tokens", "reported_tokens"].map(Alias::new),
				)
				.from(Alias::new("generation_remote_usage"))
				.and_where(Expr::cust("purpose='inference'"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(usages.len(), 2);
		let old = usages.iter().find(|usage| usage.0 == old_attempt).unwrap();
		assert_eq!(old.1, "RESERVED");
		assert_eq!(old.3, None);
		let new = usages.iter().find(|usage| usage.0 != old_attempt).unwrap();
		assert_eq!(new.1, "SETTLED");
		assert_eq!(new.3, Some(2));
		let used: i64 = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("used_tokens"))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(
			used,
			old.2 + 3,
			"uncertain input remains charged at every origin owner"
		);
	}
	let state: String = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust("attempt_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(old_attempt)
	.fetch_one(&p.b.store.pool)
	.await
	.unwrap();
	assert_eq!(state, "DISPATCHED");
	p.close().await;
}

#[rstest::rstest]
#[case("missing_usage", false)]
#[case("malformed_usage", false)]
#[case("overreported_usage", true)]
#[case("wrong_model", true)]
#[case("wrong_dimensions", true)]
#[case("oversized_response", true)]
#[tokio::test]
async fn generated_remote_embedding_validates_contract_and_retains_uncertain_charge(
	#[case] fault: &str,
	#[case] rejected: bool,
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let mut response = json!({"model":"home-vector","data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}});
	match fault {
		"missing_usage" => {
			response.as_object_mut().unwrap().remove("usage");
		}
		"malformed_usage" => response["usage"] = json!({"prompt_tokens":-1,"total_tokens":-1}),
		"overreported_usage" => {
			response["usage"] = json!({"prompt_tokens":99999999,"total_tokens":99999999})
		}
		"wrong_model" => response["model"] = json!("unapproved-model"),
		"wrong_dimensions" => response["data"][0]["embedding"] = json!([1.0]),
		"oversized_response" => response["padding"] = json!("x".repeat(1_048_577)),
		_ => unreachable!(),
	}
	*p.semantic.as_ref().unwrap().response.lock().await = Some(response);
	p.step().await;
	p.step().await;
	let run = p.run().await;
	if rejected {
		assert_eq!(run.control, "PAUSED", "{fault}: {run:?}");
		assert_eq!(
			run.pending["semantic_reason"], "provider_contract",
			"{fault}: {run:?}"
		);
		assert!(p.requests.lock().await.is_empty());
	} else {
		assert_eq!(run.control, "ACTIVE", "{fault}: {run:?}");
		assert_eq!(p.requests.lock().await.len(), 1);
	}
	let calls = p.semantic.as_ref().unwrap().requests.lock().await;
	assert_eq!(calls.len(), 5);
	let reserved = calls.last().unwrap()["input"].as_str().unwrap().len() as i64 + 1024;
	drop(calls);
	for node in [&p.a, &p.b] {
		let budget: (i64, i64) = sqlx::query_as(
			&Query::select()
				.columns(["used_tokens", "embedding_calls"].map(Alias::new))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(
			budget,
			(reserved + if rejected { 0 } else { 2 }, 1),
			"{fault} {}",
			node.config.node_id
		);
	}
	p.close().await;
}

#[rstest::rstest]
#[case("receiver_calls")]
#[case("home_tokens")]
#[case("home_catalog")]
#[case("receiver_expiry")]
#[tokio::test]
async fn generated_remote_prerequisites_stop_before_embedding_dispatch(
	#[case] fault: &str,
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	match fault {
		"receiver_calls" | "home_tokens" => {
			let (node, column) = if fault == "receiver_calls" {
				(&p.b, "embedding_call_limit")
			} else {
				(&p.a, "token_limit")
			};
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_budgets"))
					.value(
						Alias::new(column),
						if fault == "home_tokens" { 1 } else { 0 },
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&node.store.pool)
			.await
			.unwrap();
		}
		"home_catalog" => {
			assert_eq!(request(&p.aa, &p.a.config.api_token, "POST", "/api/authorization/acme/catalog", json!({"entry":{"id":"home-embedding","version":"1.0.0"},"expected_revision":1,"enabled":false})).await.0, 200);
		}
		"receiver_expiry" => {
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value(
						Alias::new("expires_at"),
						Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&p.b.store.pool)
			.await
			.unwrap();
		}
		_ => unreachable!(),
	}
	for _ in 0..3 {
		p.step().await;
		if p.run().await.control == "PAUSED" {
			break;
		}
	}
	let run = p.run().await;
	assert_eq!(run.control, "PAUSED", "{fault}: {run:?}");
	assert_ne!(run.phase, "FAILED");
	assert!(p.requests.lock().await.is_empty());
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"no query embedding may dispatch"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_memory_write_is_not_advertised_and_cannot_write_a_receiver_substitute(
	#[future(awt)]
	#[with(true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.model.force_memory_write.store(true, Ordering::Release);
	p.step().await;
	p.step().await;
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1);
	let run = p.run().await;
	assert_eq!(
		run.context["history"][0]["result"]["error"], "unavailable tool memory_write",
		"unsupported remote writes must return an explicit tool error"
	);
	for node in [&p.a, &p.b] {
		let count: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("memory"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&node.store.pool)
		.await
		.unwrap();
		assert_eq!(
			count, 0,
			"remote writes cannot use either node's local memory"
		);
	}
	p.close().await;
}
