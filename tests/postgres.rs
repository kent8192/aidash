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
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
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
    dependent.parent_id = Some(task.id);
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
            json!({"enabled":true}),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.get(&skill.id, &skill.version).await.unwrap(),
        skill
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
    assert_eq!(snapshot.messages[0]["sender"], "human");
    store.control(run.id, "cancel").await.unwrap();
    assert!(harness.worker_once().await.unwrap());
    assert_eq!(store.run(run.id).await.unwrap().phase, "CANCELLED");
    assert_eq!(store.task(task.id).await.unwrap().status, "CANCELLED");
    assert_eq!(store.task(dependency.id).await.unwrap().status, "OPEN");
    assert!(store.control(run.id, "resume").await.is_err());
    cleanup(store, &url, &schema).await;
}
