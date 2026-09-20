use aidash::{
    config::Config,
    domain::*,
    federation::Federation,
    registry::{Entry, Package, Registry, Search},
    store::Store,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use sqlx::{
    Connection, Executor,
    postgres::{PgConnection, PgPoolOptions},
};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

async fn setup() -> (Store, String, String) {
    let url = std::env::var("AIDASH_TEST_DATABASE_URL")
        .expect("set AIDASH_TEST_DATABASE_URL to a disposable PostgreSQL database");
    let schema = format!("aidash_{}", Uuid::new_v4().simple());
    let mut admin = PgConnection::connect(&url).await.unwrap();
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let search = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |conn, _| {
            let s = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {s}"))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    aidash::store::Store::migrate(&pool).await.unwrap();
    (
        Store {
            pool,
            node_id: "aidash://test".into(),
        },
        url,
        schema,
    )
}
async fn cleanup(store: Store, url: &str, schema: &str) {
    store.pool.close().await;
    let mut admin = PgConnection::connect(url).await.unwrap();
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .unwrap();
}
fn entry(kind: &str, id: &str, config: serde_json::Value) -> Entry {
    serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id,"ja":"調査"},"description":{"en":"test fixture"},"capabilities":["web.search"],"languages":["en","ja"],"config":config})).unwrap()
}
async fn seed(registry: &Registry) -> Entry {
    registry.register(entry("model","model",json!({"provider":"openai","model_id":"fixture","endpoint":"http://127.0.0.1:9999/v1","credential_env":null,"context_window":128000,"modalities":["text"],"cost":{}}))).await.unwrap();
    registry.register(entry("agent","research",json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Research","tools":[],"skills":[]}))).await.unwrap()
}
fn new_task() -> NewTask {
    NewTask {
        title: "Research".into(),
        description: "Compare Rust frameworks".into(),
        requirements: json!({"capability":"web.search","language":"ja"}),
        dependencies: vec![],
        parent_id: None,
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn concurrent_claims_dependencies_and_idempotent_completion() {
    let (store, url, schema) = setup().await;
    let registry = Registry::new(store.pool.clone());
    let agent = seed(&registry).await;
    let w = store
        .create_workspace("Research", "Compare frameworks")
        .await
        .unwrap();
    let task = store
        .create_task(w.id, &new_task(), "human", Some("create"))
        .await
        .unwrap();
    assert_eq!(
        task.id,
        store
            .create_task(w.id, &new_task(), "human", Some("create"))
            .await
            .unwrap()
            .id
    );
    let mut different = new_task();
    different.title = "Different".into();
    assert!(
        store
            .create_task(w.id, &different, "human", Some("create"))
            .await
            .is_err()
    );
    let owner = qualified_agent(&store.node_id, &agent.id, &agent.version);
    let (a, b) = tokio::join!(
        store.claim(task.id, 0, &owner, &agent),
        store.claim(task.id, 0, &owner, &agent)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let claimed = store.task(task.id).await.unwrap();
    assert_eq!(claimed.revision, 1);
    let mut dependent = new_task();
    dependent.dependencies = vec![task.id];
    // A prerequisite and its dependent are siblings, not a parent/child cycle.
    dependent.parent_id = None;
    let child = store
        .create_task(w.id, &dependent, "human", None)
        .await
        .unwrap();
    assert!(store.claim(child.id, 0, &owner, &agent).await.is_err());
    assert!(
        store
            .complete(
                task.id,
                &owner,
                "complete",
                &ArtifactInput {
                    kind: "text".into(),
                    name: "report".into(),
                    content: json!("result")
                }
            )
            .await
            .is_err()
    );
    let running = store
        .transition(task.id, claimed.revision, &owner, "RUNNING")
        .await
        .unwrap();
    assert_eq!(running.revision, 2);
    let artifact = ArtifactInput {
        kind: "text".into(),
        name: "report".into(),
        content: json!("result"),
    };
    let completed = store
        .complete(task.id, &owner, "complete", &artifact)
        .await
        .unwrap();
    let replay = store
        .complete(task.id, &owner, "complete", &artifact)
        .await
        .unwrap();
    assert_eq!(completed.revision, replay.revision);
    assert_eq!(store.snapshot(w.id).await.unwrap().artifacts.len(), 1);
    let mut bad = artifact.clone();
    bad.content = json!("different");
    assert!(
        store
            .complete(task.id, &owner, "complete", &bad)
            .await
            .is_err()
    );
    assert!(store.claim(child.id, 0, &owner, &agent).await.is_ok());
    let w2 = store.create_workspace("Other", "Other goal").await.unwrap();
    assert!(
        store
            .create_task(w2.id, &dependent, "human", None)
            .await
            .is_err()
    );
    let events = store.events(0, Some(w.id), 1000).await.unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "task.completed").count(),
        1
    );
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn lease_fencing_and_uncertain_effect_reconciliation() {
    let (store, url, schema) = setup().await;
    let registry = Registry::new(store.pool.clone());
    let agent = seed(&registry).await;
    let w = store
        .create_workspace("Journal", "Recover safely")
        .await
        .unwrap();
    let task = store
        .create_task(w.id, &new_task(), "human", None)
        .await
        .unwrap();
    let run = store
        .accept_run(&task, &store.node_id, &agent.id, &agent.version)
        .await
        .unwrap();
    let token = Uuid::new_v4();
    let leased = store.lease_run(token, 30).await.unwrap().unwrap();
    assert_eq!(leased.id, run.id);
    assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
    let first = store
        .invocation_start(
            &leased,
            token,
            "effect",
            "unsafe",
            &json!({"action":"write"}),
            false,
        )
        .await
        .unwrap();
    assert_eq!(first.status, "STARTED");
    sqlx::query("UPDATE runs SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    let new_token = Uuid::new_v4();
    let recovered = store.lease_run(new_token, 30).await.unwrap().unwrap();
    assert_eq!(recovered.pending["lease_recovered"], true);
    assert!(
        store
            .invocation_finish(&leased, token, "effect", &json!("stale"))
            .await
            .is_err()
    );
    let uncertain = store
        .invocation_start(
            &recovered,
            new_token,
            "effect",
            "unsafe",
            &json!({"action":"write"}),
            false,
        )
        .await
        .unwrap();
    assert_eq!(uncertain.status, "UNCERTAIN");
    let request = store
        .human_request(
            &recovered,
            "CONFIRMATION",
            "Verify the external result",
            "effect:reconcile",
        )
        .await
        .unwrap();
    store
        .answer(request.id, json!({"result":"verified"}))
        .await
        .unwrap();
    store
        .invocation_finish(&recovered, new_token, "effect", &json!("verified"))
        .await
        .unwrap();
    let replay = store
        .invocation_start(
            &recovered,
            new_token,
            "effect",
            "unsafe",
            &json!({"action":"write"}),
            false,
        )
        .await
        .unwrap();
    assert_eq!(replay.status, "COMPLETED");
    assert_eq!(replay.result, Some(json!("verified")));
    assert!(
        store
            .invocation_start(
                &recovered,
                new_token,
                "effect",
                "unsafe",
                &json!({"action":"other"}),
                false
            )
            .await
            .is_err()
    );
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn registry_installation_versions_and_authenticated_api() {
    let (store, url, schema) = setup().await;
    let registry = Registry::new(store.pool.clone());
    seed(&registry).await;
    let skill = entry(
        "skill",
        "research-guide",
        json!({"instructions":"Keep citations"}),
    );
    let package = Package {
        entity: skill.clone(),
        author: "Fixture".into(),
        permissions: vec![],
        dependencies: vec![],
    };
    let record = registry
        .publish(&store.pool, package.clone())
        .await
        .unwrap();
    assert!(
        registry
            .install(
                &store.pool,
                &skill.id,
                &skill.version,
                "sha256:wrong",
                json!({})
            )
            .await
            .is_err()
    );
    registry
        .install(
            &store.pool,
            &skill.id,
            &skill.version,
            &record.digest,
            json!({"instructions":"Use node-local citations"}),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.get(&skill.id, &skill.version).await.unwrap(),
        Entry {
            config: json!({"instructions":"Use node-local citations"}),
            ..skill.clone()
        }
    );
    let mut changed = package;
    changed
        .entity
        .description
        .insert("en".into(), "Changed".into());
    assert!(registry.publish(&store.pool, changed).await.is_err());
    let matches = registry
        .list(&Search {
            kind: Some("agent".into()),
            capability: Some("web.search".into()),
            language: Some("ja".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(matches.len(), 1);
    let config = Config {
        node_id: store.node_id.clone(),
        endpoint: "http://127.0.0.1:18080".into(),
        listen: "127.0.0.1:18080".parse().unwrap(),
        database_url: url.clone(),
        nats_url: "nats://127.0.0.1:42270".into(),
        api_token: "test-access-token".into(),
        web_dir: "web/dist".into(),
        lease_seconds: 30,
    };
    let f = Federation {
        store: store.clone(),
        registry,
        config,
        client: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    let router = aidash::api::router(f);
    let unauth = router
        .clone()
        .oneshot(Request::get("/api/state").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
    let auth = router
        .clone()
        .oneshot(
            Request::get("/api/state")
                .header("Authorization", "Bearer test-access-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(auth.status(), StatusCode::OK);
    let peer = router
        .oneshot(
            Request::post("/federation/v0.1/discover")
                .header("x-aidash-node", "aidash://intruder")
                .header("x-aidash-protocol", "0.1")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(peer.status(), StatusCode::UNAUTHORIZED);
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn human_requests_controls_and_cancellation_before_dependencies_finish() {
    let (store, url, schema) = setup().await;
    let registry = Registry::new(store.pool.clone());
    let agent = seed(&registry).await;
    let workspace = store
        .create_workspace("Controls", "Keep human decisions durable")
        .await
        .unwrap();
    let dependency = store
        .create_task(workspace.id, &new_task(), "human", None)
        .await
        .unwrap();
    let mut input = new_task();
    input.dependencies = vec![dependency.id];
    let task = store
        .create_task(workspace.id, &input, "human", None)
        .await
        .unwrap();
    let run = store
        .accept_run(&task, &store.node_id, &agent.id, &agent.version)
        .await
        .unwrap();
    let config = Config {
        node_id: store.node_id.clone(),
        endpoint: "http://127.0.0.1:18080".into(),
        listen: "127.0.0.1:18080".parse().unwrap(),
        database_url: url.clone(),
        nats_url: "nats://127.0.0.1:42270".into(),
        api_token: "test-access-token".into(),
        web_dir: "web/dist".into(),
        lease_seconds: 30,
    };
    let federation = Federation {
        store: store.clone(),
        registry,
        config,
        client: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    let harness = aidash::harness::Harness {
        federation: federation.clone(),
    };
    store.control(run.id, "pause").await.unwrap();
    assert!(!harness.worker_once().await.unwrap());
    assert_eq!(
        store.control(run.id, "resume").await.unwrap().control,
        "ACTIVE"
    );
    for kind in [
        "QUESTION",
        "APPROVAL_REQUIRED",
        "CONFIRMATION",
        "INFORMATION_REQUEST",
    ] {
        let request = store
            .human_request(&run, kind, "Proceed?", kind)
            .await
            .unwrap();
        assert_eq!(
            store
                .human_request(&run, kind, "Proceed?", kind)
                .await
                .unwrap()
                .id,
            request.id
        );
        assert!(
            store
                .human_request(&run, kind, "Changed?", kind)
                .await
                .is_err()
        );
        assert!(store.answer(request.id, json!(null)).await.is_err());
        assert_eq!(
            store
                .answer(request.id, json!(false))
                .await
                .unwrap()
                .response,
            Some(json!(false))
        );
        assert_eq!(
            store.answer(request.id, json!(false)).await.unwrap().id,
            request.id
        );
        assert!(store.answer(request.id, json!(true)).await.is_err());
    }
    let router = aidash::api::router(federation);
    let response = router
        .oneshot(
            Request::post(format!("/api/runs/{}/message", run.id))
                .header("Authorization", "Bearer test-access-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"content":"Keep the source citations."}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = store.snapshot(workspace.id).await.unwrap();
    assert_eq!(snapshot.messages[0].sender, "human");
    store.control(run.id, "cancel").await.unwrap();
    assert!(harness.worker_once().await.unwrap());
    assert_eq!(store.run(run.id).await.unwrap().phase, "CANCELLED");
    assert_eq!(store.task(task.id).await.unwrap().status, "CANCELLED");
    assert_eq!(store.task(dependency.id).await.unwrap().status, "OPEN");
    assert!(store.control(run.id, "resume").await.is_err());
    cleanup(store, &url, &schema).await;
}

fn federation_for(store: &Store) -> Federation {
    Federation {
        store: store.clone(),
        registry: Registry::new(store.pool.clone()),
        config: Config {
            node_id: store.node_id.clone(),
            endpoint: "http://127.0.0.1:18080".into(),
            listen: "127.0.0.1:18080".parse().unwrap(),
            database_url: String::new(),
            nats_url: "nats://127.0.0.1:1".into(),
            api_token: "test-access-token".into(),
            web_dir: "web/dist".into(),
            lease_seconds: 30,
        },
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap(),
        notify: Arc::new(tokio::sync::Notify::new()),
    }
}
async fn running_task(store: &Store, agent: &Entry, workspace: Uuid, parent: Option<Uuid>) -> Task {
    let mut input = new_task();
    input.parent_id = parent;
    let task = store
        .create_task(workspace, &input, "human", None)
        .await
        .unwrap();
    let owner = qualified_agent(&store.node_id, &agent.id, &agent.version);
    let task = store
        .claim(task.id, task.revision, &owner, agent)
        .await
        .unwrap();
    store
        .transition(task.id, task.revision, &owner, "RUNNING")
        .await
        .unwrap()
}
async fn final_response(store: &Store, task: Uuid) {
    let response = aidash::provider::ModelResponse {
        text: "Report using the available results".into(),
        ..Default::default()
    };
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE task_id=$1")
        .bind(task)
        .bind(json!({"response":response,"cursor":0}))
        .execute(&store.pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn parent_can_finish_after_explicit_child_abandonment() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    let workspace = store
        .create_workspace("Partial results", "Resolve terminal children")
        .await
        .unwrap();
    let parent = running_task(&store, &agent, workspace.id, None).await;
    let mut children = Vec::new();
    for status in ["FAILED", "BLOCKED", "CANCELLED"] {
        let child = running_task(&store, &agent, workspace.id, Some(parent.id)).await;
        let child = store
            .transition(
                child.id,
                child.revision,
                child.owner.as_deref().unwrap(),
                status,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE runs SET control='PAUSED' WHERE task_id=$1")
            .bind(child.id)
            .execute(&store.pool)
            .await
            .unwrap();
        children.push(child);
    }
    final_response(&store, parent.id).await;
    let harness = aidash::harness::Harness {
        federation: f.clone(),
    };
    harness.worker_once().await.unwrap();
    let run: Run = sqlx::query_as("SELECT * FROM runs WHERE task_id=$1")
        .bind(parent.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(run.phase, "WAITING");
    let request_id = run.pending["human_request_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for child in children {
        assert!(
            store
                .abandon_task(child.id, child.revision + 1, "No longer needed")
                .await
                .is_err()
        );
        assert!(
            store
                .abandon_task(child.id, child.revision, " ")
                .await
                .is_err()
        );
        let response = aidash::api::router(f.clone()).oneshot(Request::post(format!("/api/tasks/{}/abandon", child.id))
            .header("authorization", "Bearer test-access-token").header("content-type", "application/json")
            .body(Body::from(json!({"revision":child.revision,"reason":"Operator accepts partial results"}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(store.task(child.id).await.unwrap().status, "ABANDONED");
    }
    store
        .answer(request_id, json!("Continue with the remaining results"))
        .await
        .unwrap();
    harness.worker_once().await.unwrap();
    assert_eq!(store.run(run.id).await.unwrap().phase, "THINKING");
    final_response(&store, parent.id).await;
    harness.worker_once().await.unwrap();
    assert_eq!(store.task(parent.id).await.unwrap().status, "COMPLETED");
    assert_eq!(
        store.snapshot(workspace.id).await.unwrap().artifacts.len(),
        1
    );
    let events = store.events(0, Some(workspace.id), 1000).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "task.abandoned"
                && e.data["reason"] == "Operator accepts partial results")
            .count(),
        3
    );
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn successful_tool_retry_resets_the_next_invocation_budget() {
    let (store, url, schema) = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::post(
                    |axum::Json(input): axum::Json<serde_json::Value>| async move {
                        if input["call"] == 1 {
                            (StatusCode::OK, axum::Json(json!({"saved":true})))
                        } else {
                            (
                                StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(json!({"error":"temporary failure"})),
                            )
                        }
                    },
                ),
            ),
        )
        .await
        .unwrap();
    });
    let f = federation_for(&store);
    seed(&f.registry).await;
    f.registry.register(entry("tool", "retry-tool", json!({"transport":"http","endpoint":endpoint,"credential_env":null,"replay":"idempotent"}))).await.unwrap();
    let agent = f.registry.register(entry("agent", "retry-agent", json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Test retries","tools":[{"id":"retry-tool","version":"1.0.0"}],"skills":[]}))).await.unwrap();
    let workspace = store
        .create_workspace("Retries", "Independent budgets")
        .await
        .unwrap();
    let task = running_task(&store, &agent, workspace.id, None).await;
    let response = aidash::provider::ModelResponse {
        tool_calls: (1..=2)
            .map(|call| aidash::provider::ToolCall {
                id: call.to_string(),
                name: "plugin_0".into(),
                arguments: json!({"call":call}),
            })
            .collect(),
        ..Default::default()
    };
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2,error='prior transient error' WHERE task_id=$1")
        .bind(task.id).bind(json!({"response":response,"cursor":0,"retry_count":5})).execute(&store.pool).await.unwrap();
    let harness = aidash::harness::Harness { federation: f };
    harness.worker_once().await.unwrap();
    let run: Run = sqlx::query_as("SELECT * FROM runs WHERE task_id=$1")
        .bind(task.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(run.pending["cursor"], 1);
    assert!(run.pending.get("retry_count").is_none());
    assert!(run.error.is_none());
    harness.worker_once().await.unwrap();
    let run = store.run(run.id).await.unwrap();
    assert_eq!(run.phase, "TOOL_CALL");
    assert_eq!(run.pending["retry_count"], 1);
    assert_eq!(store.task(task.id).await.unwrap().status, "RUNNING");
    server.abort();
    cleanup(store, &url, &schema).await;
}

async fn add_test_peer(store: &Store, node: &str, endpoint: &str) {
    sqlx::query("INSERT INTO peers(node_id,endpoint,credential_env,protocol_version,enabled) VALUES($1,$2,'AIDASH_SECRET_TEST_PEER','0.1',true)")
        .bind(node).bind(endpoint).execute(&store.pool).await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL and AIDASH_SECRET_TEST_PEER; see scripts/check.sh"]
async fn failed_home_transition_survives_outage_and_worker_restart() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    let workspace = store
        .create_workspace("Remote failure", "Retry terminal delivery")
        .await
        .unwrap();
    let mut task = store
        .create_task(workspace.id, &new_task(), "human", None)
        .await
        .unwrap();
    task.status = "RUNNING".into();
    task.owner = Some(qualified_agent(&store.node_id, &agent.id, &agent.version));
    let home_task = Arc::new(std::sync::Mutex::new(task.clone()));
    let online = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let remote_task = home_task.clone();
    let available = online.clone();
    let server = tokio::spawn(async move {
        let router = axum::Router::new().route(
            "/federation/v0.1/workspace",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let task = remote_task.clone();
                let available = available.clone();
                async move {
                    if !available.load(std::sync::atomic::Ordering::SeqCst) {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            axum::Json(json!({"error":"home unavailable"})),
                        );
                    }
                    let mut task = task.lock().unwrap();
                    if body["operation"] == "transition" {
                        task.status = body["data"]["status"].as_str().unwrap().into();
                    }
                    (StatusCode::OK, axum::Json(json!(*task)))
                }
            }),
        );
        axum::serve(listener, router).await.unwrap();
    });
    add_test_peer(&store, "aidash://home", &endpoint).await;
    let run = store
        .accept_run(&task, "aidash://home", &agent.id, &agent.version)
        .await
        .unwrap();
    sqlx::query("UPDATE runs SET phase='THINKING',pending=$2 WHERE id=$1")
        .bind(run.id)
        .bind(json!({"retry_count":5}))
        .execute(&store.pool)
        .await
        .unwrap();
    aidash::harness::Harness {
        federation: f.clone(),
    }
    .worker_once()
    .await
    .unwrap();
    let pending = store.run(run.id).await.unwrap();
    assert_eq!(pending.phase, "WAITING");
    assert_eq!(pending.pending["terminal_transition"], "FAILED");
    aidash::harness::Harness {
        federation: f.clone(),
    }
    .worker_once()
    .await
    .unwrap();
    assert_eq!(store.run(run.id).await.unwrap().phase, "WAITING");
    assert_eq!(home_task.lock().unwrap().status, "RUNNING");
    online.store(true, std::sync::atomic::Ordering::SeqCst);
    sqlx::query("UPDATE runs SET pending=jsonb_set(pending,'{wake_at}',to_jsonb(now()-interval '1 second')) WHERE id=$1").bind(run.id).execute(&store.pool).await.unwrap();
    aidash::harness::Harness { federation: f }
        .worker_once()
        .await
        .unwrap();
    assert_eq!(store.run(run.id).await.unwrap().phase, "FAILED");
    assert_eq!(home_task.lock().unwrap().status, "FAILED");
    server.abort();
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and AIDASH_SECRET_TEST_PEER; see scripts/check.sh"]
async fn terminal_delegations_allow_reads_and_exact_completion_replay_only() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    let workspace = store
        .create_workspace("Revocation", "Reject stale peer writes")
        .await
        .unwrap();
    let task = store
        .create_task(workspace.id, &new_task(), "human", None)
        .await
        .unwrap();
    let peer = "aidash://peer";
    add_test_peer(&store, peer, "http://127.0.0.1:1").await;
    sqlx::query("INSERT INTO delegations(task_id,node_id,agent_id,agent_version,delivered) VALUES($1,$2,$3,$4,true)")
        .bind(task.id).bind(peer).bind(&agent.id).bind(&agent.version).execute(&store.pool).await.unwrap();
    let owner = qualified_agent(peer, &agent.id, &agent.version);
    let task = store
        .claim(task.id, task.revision, &owner, &agent)
        .await
        .unwrap();
    store
        .transition(task.id, task.revision, &owner, "RUNNING")
        .await
        .unwrap();
    let artifact = ArtifactInput {
        kind: "text".into(),
        name: "Final".into(),
        content: json!("Done"),
    };
    let key = format!("{peer}:{}:completion", task.id);
    store
        .complete(task.id, &owner, &key, &artifact)
        .await
        .unwrap();
    let router = aidash::api::router(f);
    let token = std::env::var("AIDASH_SECRET_TEST_PEER")
        .expect("set AIDASH_SECRET_TEST_PEER for peer regression tests");
    for operation in [
        "artifact",
        "create_task",
        "delegate",
        "message",
        "event",
        "human_message",
        "claim",
        "transition",
        "snapshot",
        "task",
        "complete",
    ] {
        let data = if operation == "complete" {
            json!({"key":"completion","artifact":artifact})
        } else {
            json!({"key":"stale","content":"stale write","status":"RUNNING"})
        };
        let response = router.clone().oneshot(Request::post("/federation/v0.1/workspace")
            .header("authorization", format!("Bearer {token}")).header("x-aidash-node", peer).header("x-aidash-protocol", "0.1").header("content-type", "application/json")
            .body(Body::from(json!({"task_id":task.id,"agent":{"id":agent.id,"version":agent.version},"operation":operation,"data":data}).to_string())).unwrap()).await.unwrap();
        assert_eq!(
            response.status(),
            if matches!(operation, "snapshot" | "task" | "complete") {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            },
            "{operation}"
        );
    }
    assert_eq!(
        store.snapshot(workspace.id).await.unwrap().artifacts.len(),
        1
    );
    let mut malformed = new_task();
    malformed.requirements = json!({"capabilty":"web.search"});
    assert!(
        store
            .create_task(workspace.id, &malformed, "human", None)
            .await
            .is_err()
    );
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn queued_executor_conflict_rolls_back_claim_and_dependencies_wait() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    let mut other = agent.clone();
    other.id = "other".into();
    f.registry.register(other.clone()).await.unwrap();
    let workspace = store
        .create_workspace("Claims", "Do not strand work")
        .await
        .unwrap();
    let prerequisite = running_task(&store, &agent, workspace.id, None).await;
    sqlx::query("UPDATE runs SET control='PAUSED' WHERE task_id=$1")
        .bind(prerequisite.id)
        .execute(&store.pool)
        .await
        .unwrap();
    let mut input = new_task();
    input.dependencies = vec![prerequisite.id];
    let task = store
        .create_task(workspace.id, &input, "human", None)
        .await
        .unwrap();
    let run = store
        .accept_run(&task, &store.node_id, &agent.id, &agent.version)
        .await
        .unwrap();
    let harness = aidash::harness::Harness {
        federation: f.clone(),
    };
    harness.worker_once().await.unwrap();
    assert_eq!(store.run(run.id).await.unwrap().phase, "WAITING");
    assert_eq!(store.task(task.id).await.unwrap().status, "OPEN");
    store
        .complete(
            prerequisite.id,
            prerequisite.owner.as_deref().unwrap(),
            "prerequisite",
            &ArtifactInput {
                kind: "text".into(),
                name: "done".into(),
                content: json!("done"),
            },
        )
        .await
        .unwrap();
    let owner = qualified_agent(&store.node_id, &other.id, &other.version);
    assert!(matches!(
        store.claim(task.id, task.revision, &owner, &other).await,
        Err(aidash::Error::Conflict(_))
    ));
    let unchanged = store.task(task.id).await.unwrap();
    assert_eq!(unchanged.revision, task.revision);
    assert!(unchanged.owner.is_none());
    sqlx::query("UPDATE runs SET pending=jsonb_set(pending,'{wake_at}',to_jsonb(now()-interval '1 second')) WHERE id=$1").bind(run.id).execute(&store.pool).await.unwrap();
    harness.worker_once().await.unwrap();
    harness.worker_once().await.unwrap();
    assert_eq!(store.task(task.id).await.unwrap().status, "RUNNING");
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn child_creation_and_parent_completion_are_serialized() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    let workspace = store
        .create_workspace("Children", "Atomic completion")
        .await
        .unwrap();
    let parent = running_task(&store, &agent, workspace.id, None).await;
    let mut input = new_task();
    input.parent_id = Some(parent.id);
    let artifact = ArtifactInput {
        kind: "text".into(),
        name: "parent".into(),
        content: json!("done"),
    };
    let (child, completed) = tokio::join!(
        store.create_task(workspace.id, &input, "human", Some("child")),
        store.complete(
            parent.id,
            parent.owner.as_deref().unwrap(),
            "parent",
            &artifact
        )
    );
    assert_ne!(child.is_ok(), completed.is_ok());
    if let Ok(child) = child {
        assert_eq!(store.task(parent.id).await.unwrap().status, "RUNNING");
        let cancelled = store
            .transition(child.id, child.revision, "human", "CANCELLED")
            .await
            .unwrap();
        store
            .abandon_task(child.id, cancelled.revision, "No longer required")
            .await
            .unwrap();
        store
            .complete(
                parent.id,
                parent.owner.as_deref().unwrap(),
                "parent",
                &artifact,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .create_task(workspace.id, &input, "human", Some("child"))
                .await
                .unwrap()
                .id,
            child.id
        );
    }
    assert!(
        store
            .create_task(workspace.id, &input, "human", Some("new-child"))
            .await
            .is_err()
    );
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn unavailable_tools_are_results_and_child_gating_advances_step() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    let workspace = store
        .create_workspace("Recovery", "Repair model errors")
        .await
        .unwrap();
    let parent = running_task(&store, &agent, workspace.id, None).await;
    let response = aidash::provider::ModelResponse {
        tool_calls: vec![aidash::provider::ToolCall {
            id: "bad".into(),
            name: "missing_tool".into(),
            arguments: json!({}),
        }],
        ..Default::default()
    };
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE task_id=$1")
        .bind(parent.id)
        .bind(json!({"response":response,"cursor":0}))
        .execute(&store.pool)
        .await
        .unwrap();
    let harness = aidash::harness::Harness {
        federation: f.clone(),
    };
    harness.worker_once().await.unwrap();
    let run: Run = sqlx::query_as("SELECT * FROM runs WHERE task_id=$1")
        .bind(parent.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(run.pending["cursor"], 1);
    assert!(
        run.context["history"][0]["result"]["error"]
            .as_str()
            .unwrap()
            .contains("missing_tool")
    );
    let mut input = new_task();
    input.parent_id = Some(parent.id);
    let child = store
        .create_task(workspace.id, &input, "human", None)
        .await
        .unwrap();
    final_response(&store, parent.id).await;
    harness.worker_once().await.unwrap();
    let waiting = store.run(run.id).await.unwrap();
    assert_eq!(waiting.phase, "WAITING");
    assert_eq!(waiting.step, run.step + 1);
    let child = store
        .transition(child.id, child.revision, "human", "CANCELLED")
        .await
        .unwrap();
    store
        .abandon_task(child.id, child.revision, "No longer required")
        .await
        .unwrap();
    let response = aidash::provider::ModelResponse {
        text: "A different final result after the child settled".into(),
        ..Default::default()
    };
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE id=$1")
        .bind(run.id)
        .bind(json!({"response":response,"cursor":0}))
        .execute(&store.pool)
        .await
        .unwrap();
    harness.worker_once().await.unwrap();
    assert_eq!(store.task(parent.id).await.unwrap().status, "COMPLETED");
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn peer_disable_and_retry_rotation_do_not_require_a_live_peer() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    add_test_peer(&store, "aidash://offline", "http://127.0.0.1:1").await;
    sqlx::query(
        "UPDATE peers SET credential_env='AIDASH_SECRET_REMOVED' WHERE node_id='aidash://offline'",
    )
    .execute(&store.pool)
    .await
    .unwrap();
    let disabled = f
        .register_peer(aidash::federation::Peer {
            node_id: "aidash://offline".into(),
            endpoint: "http://127.0.0.1:1".into(),
            credential_env: "AIDASH_SECRET_REMOVED".into(),
            protocol_version: "0.1".into(),
            enabled: false,
        })
        .await
        .unwrap();
    assert!(!disabled.enabled);
    let workspace = store
        .create_workspace("Retry", "Healthy peers progress")
        .await
        .unwrap();
    let mut healthy = None;
    for index in 0..101 {
        let task = store
            .create_task(workspace.id, &new_task(), "human", None)
            .await
            .unwrap();
        let node = if index == 100 {
            &store.node_id
        } else {
            "aidash://offline"
        };
        sqlx::query("INSERT INTO delegations(task_id,node_id,agent_id,agent_version,created_at,next_attempt_at) VALUES($1,$2,$3,$4,now()+make_interval(secs=>$5),now()-interval '1 second')")
            .bind(task.id).bind(node).bind(&agent.id).bind(&agent.version).bind(index as f64).execute(&store.pool).await.unwrap();
        if index == 100 {
            healthy = Some(task.id);
        }
    }
    f.retry_deliveries().await.unwrap();
    f.retry_deliveries().await.unwrap();
    let delivered: bool = sqlx::query_scalar("SELECT delivered FROM delegations WHERE task_id=$1")
        .bind(healthy)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert!(delivered);
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn registry_event_and_conversation_creation_roll_back_as_units() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    seed(&f.registry).await;
    let app = aidash::api::router(f);
    // Simulate an event insertion failure after the primary mutation.
    sqlx::query("ALTER TABLE events ADD CONSTRAINT reject_registration CHECK(kind <> 'registry.registered')").execute(&store.pool).await.unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/registry")
                .header("authorization", "Bearer test-access-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&entry(
                        "skill",
                        "rollback",
                        json!({"instructions":"test"}),
                    ))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        Registry::new(store.pool.clone())
            .get("rollback", "1.0.0")
            .await
            .is_err()
    );
    sqlx::query("ALTER TABLE events ADD CONSTRAINT reject_conversation CHECK(kind <> 'conversation.created')").execute(&store.pool).await.unwrap();
    let response=app.oneshot(Request::post("/api/conversations").header("authorization","Bearer test-access-token").header("content-type","application/json").body(Body::from(json!({"title":"Atomic","goal":"Must roll back","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(store.workspaces().await.unwrap().is_empty());
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and NATS"]
async fn rejected_outbox_payload_does_not_block_later_events() {
    let (mut store, url, schema) = setup().await;
    store.node_id = format!("aidash://outbox-{}", Uuid::new_v4().simple());
    let mut f = federation_for(&store);
    f.config.nats_url = std::env::var("AIDASH_TEST_NATS_URL").unwrap_or_else(|_| {
        format!(
            "nats://127.0.0.1:{}",
            std::env::var("AIDASH_NATS_PORT").unwrap_or_else(|_| "42270".into())
        )
    });
    let bus = aidash::bus::EventBus::connect(&f.config.nats_url, &store.node_id)
        .await
        .unwrap();
    let poison = store
        .emit(None, "large", json!({"text":"x".repeat(2_000_000)}))
        .await
        .unwrap();
    let healthy = store.emit(None, "small", json!({"ok":true})).await.unwrap();
    assert_eq!(bus.publish_once(&f).await.unwrap(), 1);
    let row: (bool, Option<String>) =
        sqlx::query_as("SELECT published_at IS NOT NULL,publish_error FROM events WHERE id=$1")
            .bind(poison.id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert!(!row.0);
    assert!(row.1.is_some());
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT published_at IS NOT NULL FROM events WHERE id=$1")
            .bind(healthy.id)
            .fetch_one(&store.pool)
            .await
            .unwrap()
    );
    bus.context.delete_stream(&bus.stream_name).await.unwrap();
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn ambiguous_peer_credentials_cannot_impersonate_another_node() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let secret = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
    add_test_peer(&store, "aidash://peer-b", "http://127.0.0.1:1").await;
    assert!(
        f.authenticate_peer("aidash://peer-b", &secret)
            .await
            .is_ok()
    );
    add_test_peer(&store, "aidash://peer-c", "http://127.0.0.1:1").await;
    assert!(
        f.authenticate_peer("aidash://peer-c", &secret)
            .await
            .is_err()
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/.well-known/aidash",
                axum::routing::get(|| async {
                    axum::Json(json!({"id":"aidash://peer-d","protocol_version":"0.1"}))
                }),
            ),
        )
        .await
        .unwrap();
    });
    assert!(matches!(
        f.register_peer(aidash::federation::Peer {
            node_id: "aidash://peer-d".into(),
            endpoint,
            credential_env: "AIDASH_SECRET_TEST_PEER".into(),
            protocol_version: "0.1".into(),
            enabled: true
        })
        .await,
        Err(aidash::Error::Invalid(_))
    ));
    server.abort();
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn recovery_publishes_reconciliation_marker_with_the_request() {
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let mut agent = seed(&f.registry).await;
    f.registry
        .register(entry(
            "tool",
            "unsafe-http",
            json!({"transport":"http","endpoint":"http://127.0.0.1:9","replay":"unsafe"}),
        ))
        .await
        .unwrap();
    agent.id = "unsafe-agent".into();
    agent.config["tools"] = json!([{"id":"unsafe-http","version":"1.0.0"}]);
    f.registry.register(agent.clone()).await.unwrap();
    let workspace = store
        .create_workspace("Reconcile", "Never expose an incomplete request")
        .await
        .unwrap();
    let task = running_task(&store, &agent, workspace.id, None).await;
    let response = aidash::provider::ModelResponse {
        tool_calls: vec![aidash::provider::ToolCall {
            id: "unsafe".into(),
            name: "plugin_0".into(),
            arguments: json!({}),
        }],
        ..Default::default()
    };
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE task_id=$1")
        .bind(task.id)
        .bind(json!({"response":response,"cursor":0}))
        .execute(&store.pool)
        .await
        .unwrap();
    let worker = Uuid::new_v4();
    let run = store.lease_run(worker, 30).await.unwrap().unwrap();
    let key = format!("{}:0:0", run.id);
    store
        .invocation_start(&run, worker, &key, "plugin_0", &json!({}), false)
        .await
        .unwrap();
    sqlx::query("UPDATE runs SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(run.id)
        .execute(&store.pool)
        .await
        .unwrap();
    aidash::harness::Harness { federation: f }
        .worker_once()
        .await
        .unwrap();
    let current = store.run(run.id).await.unwrap();
    let request = current.pending["human_request_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(current.pending["uncertain_key"], key);
    assert!(matches!(
        store.answer(request, json!("done")).await,
        Err(aidash::Error::Invalid(_))
    ));
    store
        .answer(request, json!({"result":"verified"}))
        .await
        .unwrap();
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn agent_tools_attach_children_and_clusters_require_existing_agents() {
    use aidash::tool::{PluginTool, Tool, ToolConfig, ToolContext};
    let (store, url, schema) = setup().await;
    let f = federation_for(&store);
    let agent = seed(&f.registry).await;
    for coordinator in ["missing", "model"] {
        assert!(
            f.registry
                .register(entry(
                    "cluster",
                    coordinator,
                    json!({"coordinator":{"id":coordinator,"version":"1.0.0"}})
                ))
                .await
                .is_err()
        );
    }
    f.registry
        .register(entry(
            "cluster",
            "valid-cluster",
            json!({"coordinator":{"id":"research","version":"1.0.0"}}),
        ))
        .await
        .unwrap();
    let workspace = store
        .create_workspace("Agent tool", "Await delegated results")
        .await
        .unwrap();
    let task = running_task(&store, &agent, workspace.id, None).await;
    let run: Run = sqlx::query_as("SELECT * FROM runs WHERE task_id=$1")
        .bind(task.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    let config = ToolConfig::Agent {
        node_id: store.node_id.clone(),
        agent: aidash::registry::EntityRef {
            id: agent.id,
            version: agent.version,
        },
    };
    let plugin = PluginTool {
        entry: entry(
            "tool",
            "delegate-tool",
            serde_json::to_value(&config).unwrap(),
        ),
        alias: "plugin_0".into(),
        config,
        client: f.client.clone(),
    };
    let context = ToolContext {
        home: aidash::federation::Home::new(f, run.clone()),
        store: store.clone(),
        run,
    };
    for (index, parent) in [None, Some(serde_json::Value::Null)]
        .into_iter()
        .enumerate()
    {
        let mut input = json!({"title":"Child","description":"Delegated work"});
        if let Some(parent) = parent {
            input["parent_id"] = parent;
        }
        let result = plugin
            .invoke(&context, input, &format!("delegated-{index}"))
            .await
            .unwrap();
        assert_eq!(result["task"]["parent_id"], task.id.to_string());
    }
    cleanup(store, &url, &schema).await;
}
