use aidash::{
    api, config::Config, domain::qualified_agent, federation::Federation, harness::Harness,
    registry::Registry, store::Store,
};
use axum::{Json, Router, body::Body, http::Request, routing::post};
use serde_json::{Value, json};
use sqlx::{
    Connection, Executor,
    postgres::{PgConnection, PgPoolOptions},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;
use uuid::Uuid;

async fn request(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    value: Value,
) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn setup() -> (Federation, String, String) {
    let url = std::env::var("AIDASH_TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    let schema = format!("execution_{}", Uuid::new_v4().simple());
    let mut admin = PgConnection::connect(&url).await.unwrap();
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let search = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .after_connect(move |connection, _| {
            let schema = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {schema}"))
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SELECT set_config('application_name',$1,false)")
                    .bind(&schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let store = Store {
        pool: pool.clone(),
        node_id: "aidash://execution-test".into(),
    };
    let federation = Federation {
        store,
        registry: Registry::new(pool),
        config: Config {
            node_id: "aidash://execution-test".into(),
            endpoint: "http://localhost:8080".into(),
            listen: "127.0.0.1:0".parse().unwrap(),
            database_url: url.clone(),
            nats_url: "nats://127.0.0.1:4222".into(),
            api_token: "operator-execution-fixture".into(),
            web_dir: "web/dist".into(),
            lease_seconds: 30,
        },
        client: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    (federation, url, schema)
}

async fn cleanup(f: Federation, url: &str, schema: &str) {
    f.store.pool.close().await;
    PgConnection::connect(url)
        .await
        .unwrap()
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .unwrap();
}

fn policy(node: &str) -> Value {
    let agent = qualified_agent(node, "research", "1.0.0");
    json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},agent:{"kind":"agent"}},
        "policies":[{"id":"approved-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})
}

async fn bootstrap(f: &Federation, app: &Router, endpoint: &str) -> (Value, String, Uuid) {
    let operator = &f.config.api_token;
    let policy = policy(&f.config.node_id);
    assert_eq!(
        request(
            app,
            operator,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":0,"bundle":policy})
        )
        .await
        .0,
        200
    );
    for (kind, id, config) in [
        (
            "model",
            "model",
            json!({"provider":"openai","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"modalities":["text"],"cost":{}}),
        ),
        (
            "tool",
            "http",
            json!({"transport":"http","endpoint":format!("{endpoint}/effect"),"credential_env":null,"replay":"idempotent"}),
        ),
        (
            "agent",
            "research",
            json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Test approved work","tools":[{"id":"http","version":"1.0.0"}],"skills":[]}),
        ),
    ] {
        let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
        assert_eq!(
            request(app, operator, "POST", "/api/registry", entry)
                .await
                .0,
            200
        );
        let (status, response) = request(
            app,
            operator,
            "POST",
            "/api/authorization/acme/catalog",
            json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}),
        )
        .await;
        assert_eq!(status, 200, "catalog admission: {response}");
    }
    let (status, credential) = request(
        app,
        operator,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    assert_eq!(status, 200);
    let token = credential["token"].as_str().unwrap().to_owned();
    let (status, workspace) = request(
        app,
        &token,
        "POST",
        "/api/workspaces",
        json!({"title":"Approved task","goal":"Use exactly one tool"}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, task) = request(
        app,
        &token,
        "POST",
        &format!(
            "/api/workspaces/{}/tasks",
            workspace["id"].as_str().unwrap()
        ),
        json!({"title":"Research","description":"Use approved tools"}),
    )
    .await;
    assert_eq!(status, 200);
    (
        policy,
        token,
        Uuid::parse_str(task["id"].as_str().unwrap()).unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority()
 {
    let (f, url, schema) = setup().await;
    let effects = Arc::new(AtomicUsize::new(0));
    let counter = effects.clone();
    let server=Router::new().route("/effect",post(move || { let counter=counter.clone(); async move {
        counter.fetch_add(1,Ordering::SeqCst); Json(json!({"saved":true}))
    }})).route("/v1/chat/completions",post(|Json(body):Json<Value>| async move {
        let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        let message=if context["history"].as_array().unwrap().iter().any(|e| e["kind"]=="tool") {
            json!({"role":"assistant","content":"Completed authorized work"})
        } else { json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"plugin_0","arguments":"{}"}}]}) };
        Json(json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, server).await.unwrap();
    });
    let app = api::router(f.clone());
    let (mut policy, token, task_id) = bootstrap(&f, &app, &endpoint).await;
    let (status, _) = request(
        &app,
        &token,
        "POST",
        &format!("/api/tasks/{task_id}/claim"),
        json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a scoped claim must persist an execution identity"
    );
    let harness = Harness {
        federation: f.clone(),
    };
    assert!(harness.worker_once().await.unwrap());
    assert!(harness.worker_once().await.unwrap());
    let run = f.store.runs().await.unwrap().remove(0);
    assert_eq!(run.phase, "TOOL_CALL");
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let agent = qualified_agent(&f.config.node_id, "research", "1.0.0");
    policy["subjects"][&agent]["enabled"] = json!(false);
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
    assert!(harness.worker_once().await.unwrap());
    let paused = f.store.run(run.id).await.unwrap();
    assert_eq!(paused.control, "PAUSED");
    assert_eq!(
        paused.pending, run.pending,
        "revocation must not discard the durable tool cursor"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(!harness.worker_once().await.unwrap());
    policy["subjects"][&agent]["enabled"] = json!(true);
    policy["policies"].as_array_mut().unwrap().push(json!({"id":"agent-tool-deny","effect":"deny","subjects":{"ids":[agent]},"actions":["tool.invoke"],"resources":{"kinds":["tool"]}}));
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
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/runs/{}/control", run.id),
            json!({"action":"resume"})
        )
        .await
        .0,
        200
    );
    assert!(harness.worker_once().await.unwrap());
    assert_eq!(f.store.run(run.id).await.unwrap().control, "PAUSED");
    assert_eq!(
        effects.load(Ordering::SeqCst),
        0,
        "root allow must not override agent deny"
    );
    policy["policies"].as_array_mut().unwrap().pop();
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
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/runs/{}/control", run.id),
            json!({"action":"resume"})
        )
        .await
        .0,
        200
    );
    let restarted = Harness {
        federation: f.clone(),
    };
    for _ in 0..8 {
        restarted.worker_once().await.unwrap();
        if f.store.run(run.id).await.unwrap().phase == "COMPLETED" {
            break;
        }
    }
    assert_eq!(f.store.run(run.id).await.unwrap().phase, "COMPLETED");
    assert_eq!(f.store.task(task_id).await.unwrap().status, "COMPLETED");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.store
            .snapshot(run.workspace_id)
            .await
            .unwrap()
            .artifacts
            .len(),
        1
    );
    server.abort();
    let _ = server.await;
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn catalog_approval_and_run_read_denials_cover_search_collections_and_event_replay() {
    use futures_util::StreamExt;
    use std::time::Duration;
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
    assert_eq!(
        request(&app, &token, "GET", "/api/registry", Value::Null)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        request(&app, &token, "POST", "/api/discover", json!({}))
            .await
            .1["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
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
    let (_, other_credential) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/authorization/other/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    let other_token = other_credential["token"].as_str().unwrap();
    assert_eq!(
        request(&app, other_token, "GET", "/api/registry", Value::Null).await,
        (200, json!([]))
    );
    assert_eq!(
        request(
            &app,
            other_token,
            "GET",
            "/api/registry/research/1.0.0",
            Value::Null
        )
        .await
        .0,
        403
    );
    let catalog =
        json!({"entry":{"id":"research","version":"1.0.0"},"enabled":false,"expected_revision":1});
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/authorization/acme/catalog",
            catalog.clone()
        )
        .await
        .0,
        200
    );
    let claim = json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}});
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/tasks/{task_id}/claim"),
            claim.clone()
        )
        .await
        .0,
        403
    );
    assert_eq!(f.store.task(task_id).await.unwrap().status, "OPEN");
    assert!(f.store.runs().await.unwrap().is_empty());
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            "/api/discover",
            json!({"query":"research"})
        )
        .await
        .1["agents"],
        json!([])
    );
    assert_eq!(request(&app,&f.config.api_token,"POST","/api/authorization/acme/catalog",json!({"entry":{"id":"research","version":"1.0.0"},"enabled":true,"expected_revision":2})).await.0,200);
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/authorization/acme/catalog",
            catalog
        )
        .await
        .0,
        409
    );
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/tasks/{task_id}/claim"),
            claim
        )
        .await
        .0,
        200
    );
    let run = f.store.runs().await.unwrap().remove(0);
    sqlx::query("UPDATE runs SET context=$2 WHERE id=$1")
        .bind(run.id)
        .bind(json!({"private":"unreadable-run-journal"}))
        .execute(&f.store.pool)
        .await
        .unwrap();
    for path in [
        format!("/api/tasks/{task_id}/claim"),
        format!("/api/tasks/{task_id}/delegate"),
    ] {
        assert_eq!(request(&app,other_token,"POST",&path,json!({"revision":0,"node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}})).await.0,403,
            "foreign task status must not be exposed through an admission conflict");
    }
    f.store
        .human_request(
            &run,
            "QUESTION",
            "unreadable-run-journal",
            "private-question",
        )
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT max(sequence) FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO events(id,node_id,workspace_id,kind,data) SELECT gen_random_uuid(),$1,$2,'model.completed',$3 FROM generate_series(1,501)")
        .bind(&f.config.node_id).bind(run.workspace_id).bind(json!({"run_id":run.id,"private":"unreadable-run-journal"})).execute(&f.store.pool).await.unwrap();
    f.store
        .message(
            run.workspace_id,
            "alice",
            "permitted message after rejected events",
            None,
        )
        .await
        .unwrap();
    policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-run-read","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["run"],"ids":[run.id.to_string()]}}));
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
        format!("/api/workspaces/{}", run.workspace_id),
        format!("/api/events?after={after}"),
    ] {
        let (status, value) = request(&app, &token, "GET", &path, Value::Null).await;
        assert_eq!(status, 200, "{path}: {value}");
        assert!(
            !value.to_string().contains("unreadable-run-journal"),
            "leak through {path}"
        );
        if path == "/api/state" {
            assert_eq!(value["runs"], json!([]));
            assert_eq!(value["human_requests"], json!([]));
        }
        if path.starts_with("/api/events?") {
            assert_eq!(
                value.as_array().unwrap().len(),
                1,
                "hidden events must not trap pagination"
            );
        }
    }
    let response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/events/stream?workspace_id={}&after={after}",
                run.workspace_id
            ))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut stream = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&frame);
    assert!(text.contains("permitted message after rejected events"));
    assert!(!text.contains("unreadable-run-journal"));
    drop(stream);
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/runs/{}/control", run.id),
            json!({"action":"pause"})
        )
        .await
        .0,
        403,
        "control responses must not expose a denied run journal"
    );
    assert_eq!(f.store.run(run.id).await.unwrap().control, "ACTIVE");
    f.store.control(run.id, "pause").await.unwrap();
    let (_, second) = request(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{}/tasks", run.workspace_id),
        json!({"title":"observe","description":"inspect the permitted workspace"}),
    )
    .await;
    let second_id: Uuid = second["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/tasks/{second_id}/claim"),
            json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
        )
        .await
        .0,
        200
    );
    let observer = f
        .store
        .runs()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.task_id == second_id)
        .unwrap();
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE id=$1")
        .bind(observer.id).bind(json!({"response":{"text":"","tool_calls":[{"id":"observe","name":"workspace_observe","arguments":{}}],"input_tokens":0,"output_tokens":0},"cursor":0}))
        .execute(&f.store.pool).await.unwrap();
    Harness {
        federation: f.clone(),
    }
    .worker_once()
    .await
    .unwrap();
    let result: Value = sqlx::query_scalar("SELECT result FROM invocations WHERE run_id=$1")
        .bind(observer.id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert!(
        !result.to_string().contains("unreadable-run-journal"),
        "worker observation must apply the same run visibility as API snapshots"
    );
    assert!(
        result
            .to_string()
            .contains("permitted message after rejected events")
    );
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn child_execution_retains_parent_authority_and_supports_credential_rotation_and_cancellation()
 {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
    let child = qualified_agent(&f.config.node_id, "child", "1.0.0");
    policy["subjects"][&child] = json!({"kind":"agent"});
    let parent = qualified_agent(&f.config.node_id, "research", "1.0.0");
    policy["policies"].as_array_mut().unwrap().push(json!({"id":"parent-tool-deny","effect":"deny","subjects":{"ids":[parent]},"actions":["tool.invoke"],"resources":{"kinds":["tool"],"ids":["http"]}}));
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
    let mut entry =
        serde_json::to_value(f.registry.get("research", "1.0.0").await.unwrap()).unwrap();
    entry["id"] = json!("child");
    assert_eq!(
        request(&app, &f.config.api_token, "POST", "/api/registry", entry)
            .await
            .0,
        200
    );
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/authorization/acme/catalog",
            json!({"entry":{"id":"child","version":"1.0.0"},"expected_revision":0,"enabled":true})
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
            &format!("/api/tasks/{task_id}/claim"),
            json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
        )
        .await
        .0,
        200
    );
    let harness = Harness {
        federation: f.clone(),
    };
    harness.worker_once().await.unwrap();
    let parent_run = f.store.runs().await.unwrap().remove(0);
    let (_, task) = request(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{}/tasks", parent_run.workspace_id),
        json!({"title":"Child","description":"Delegated work","parent_id":task_id}),
    )
    .await;
    let child_task = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    let response = json!({"text":"","tool_calls":[{"id":"delegate","name":"task_delegate","arguments":{"task_id":child_task,"node_id":f.config.node_id,"agent":{"id":"child","version":"1.0.0"}}}],"input_tokens":0,"output_tokens":0});
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE id=$1")
        .bind(parent_run.id)
        .bind(json!({"response":response,"cursor":0}))
        .execute(&f.store.pool)
        .await
        .unwrap();
    harness.worker_once().await.unwrap();
    let child_run = f
        .store
        .runs()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.task_id == child_task)
        .expect("parent tool must admit a child run");
    let chain: Vec<String> =
        sqlx::query_scalar("SELECT subject_chain FROM authorization_execution WHERE run_id=$1")
            .bind(child_run.id)
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
    assert_eq!(chain, vec!["alice".to_owned(), parent, child]);
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/runs/{}/control", parent_run.id),
            json!({"action":"pause"})
        )
        .await
        .0,
        200
    );
    harness.worker_once().await.unwrap();
    let response = json!({"text":"","tool_calls":[{"id":"effect","name":"plugin_0","arguments":{}}],"input_tokens":0,"output_tokens":0});
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE id=$1")
        .bind(child_run.id)
        .bind(json!({"response":response,"cursor":0}))
        .execute(&f.store.pool)
        .await
        .unwrap();
    Harness {
        federation: f.clone(),
    }
    .worker_once()
    .await
    .unwrap();
    assert_eq!(
        f.store.run(child_run.id).await.unwrap().control,
        "PAUSED",
        "parent deny must intersect the child grant after restart"
    );
    let invocations: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations WHERE run_id=$1")
        .bind(child_run.id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(
        invocations, 0,
        "authorization must precede durable invocation admission"
    );
    let (_, credentials) = request(
        &app,
        &f.config.api_token,
        "GET",
        "/api/authorization/acme/credentials",
        Value::Null,
    )
    .await;
    let old_id = credentials[0]["id"].as_str().unwrap();
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            &format!("/api/authorization/acme/credentials/{old_id}/revoke"),
            json!({})
        )
        .await
        .0,
        200
    );
    let (_, fresh) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    assert_eq!(
        request(
            &app,
            fresh["token"].as_str().unwrap(),
            "POST",
            &format!("/api/runs/{}/control", child_run.id),
            json!({"action":"resume"})
        )
        .await
        .0,
        200
    );
    let credential: Uuid =
        sqlx::query_scalar("SELECT credential_id FROM authorization_execution WHERE run_id=$1")
            .bind(child_run.id)
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
    assert_eq!(
        credential.to_string(),
        fresh["credential"]["id"].as_str().unwrap()
    );
    // Operator cancellation is cleanup and must remain possible after every
    // source credential has been revoked; it must never invoke the provider.
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            &format!("/api/authorization/acme/credentials/{credential}/revoke"),
            json!({})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            &format!("/api/runs/{}/control", child_run.id),
            json!({"action":"cancel"})
        )
        .await
        .0,
        200
    );
    harness.worker_once().await.unwrap();
    assert_eq!(f.store.run(child_run.id).await.unwrap().phase, "CANCELLED");
    assert_eq!(f.store.task(child_task).await.unwrap().status, "CANCELLED");
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect() {
    use std::time::Duration;
    let (f, url, schema) = setup().await;
    let worker_federation = f.for_workers().await.unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let handler_entered = entered.clone();
    let handler_release = release.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Router::new().route(
        "/effect",
        post(move || {
            let entered = handler_entered.clone();
            let release = handler_release.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                Json(json!({"effect":"committed"}))
            }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, fixture).await.unwrap();
    });
    let app = api::router(f.clone());
    let (mut policy, token, task_id) = bootstrap(&f, &app, &endpoint).await;
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/tasks/{task_id}/claim"),
            json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
        )
        .await
        .0,
        200
    );
    let harness = Harness {
        federation: worker_federation.clone(),
    };
    harness.worker_once().await.unwrap();
    let run = f.store.runs().await.unwrap().remove(0);
    let response = json!({"text":"","tool_calls":[{"id":"effect","name":"plugin_0","arguments":{}}],"input_tokens":0,"output_tokens":0});
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE id=$1")
        .bind(run.id)
        .bind(json!({"response":response,"cursor":0}))
        .execute(&f.store.pool)
        .await
        .unwrap();
    let worker = tokio::spawn(async move { harness.worker_once().await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let audited:i64=sqlx::query_scalar("SELECT count(*) FROM authorization_decisions WHERE action='tool.invoke' AND resource_id='http' AND decision->>'allowed'='true'")
        .fetch_one(&f.store.pool).await.unwrap();
    assert_eq!(
        audited, 2,
        "root and agent authorization must be durable before the HTTP effect starts"
    );
    let (_, credentials) = request(
        &app,
        &f.config.api_token,
        "GET",
        "/api/authorization/acme/credentials",
        Value::Null,
    )
    .await;
    let revoke_path = format!(
        "/api/authorization/acme/credentials/{}/revoke",
        credentials[0]["id"].as_str().unwrap()
    );
    policy["subjects"][qualified_agent(&f.config.node_id, "research", "1.0.0")]["enabled"] =
        json!(false);
    let mut replacements = tokio::task::JoinSet::new();
    // Fill every API connection with a waiting revocation. The worker must
    // still persist its result and release the lease using its separate pool.
    for _ in 0..11 {
        let app = app.clone();
        let operator = f.config.api_token.clone();
        let policy = policy.clone();
        replacements.spawn(async move {
            request(
                &app,
                &operator,
                "POST",
                "/api/authorization/acme",
                json!({"expected_revision":1,"bundle":policy}),
            )
            .await
        });
    }
    let revoke = tokio::spawn({
        let app = app.clone();
        let operator = f.config.api_token.clone();
        async move { request(&app, &operator, "POST", &revoke_path, json!({})).await }
    });
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            let waiting:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND (query LIKE 'UPDATE authorization_bundles%' OR query LIKE 'UPDATE authorization_credentials%')")
                .bind(&schema).fetch_one(&worker_federation.store.pool).await.unwrap();
            if waiting==12 {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("revocation was not serialized with the in-flight effect boundary");
    release.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    );
    let mut replaced = 0;
    while let Some(result) = replacements.join_next().await {
        match result.unwrap().0 {
            200 => replaced += 1,
            409 => (),
            status => panic!("unexpected replacement status: {status}"),
        }
    }
    assert_eq!(replaced, 1);
    assert_eq!(revoke.await.unwrap().0, 200);
    let invocation: String = sqlx::query_scalar("SELECT status FROM invocations WHERE run_id=$1")
        .bind(run.id)
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(invocation, "COMPLETED");
    Harness {
        federation: worker_federation.clone(),
    }
    .worker_once()
    .await
    .unwrap();
    let paused = f.store.run(run.id).await.unwrap();
    assert_eq!(paused.control, "PAUSED");
    assert_eq!(paused.pending["cursor"], 1);
    server.abort();
    let _ = server.await;
    worker_federation.store.pool.close().await;
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_delegation_requires_permission_before_atomic_admission() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
    policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-delegation","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.delegate"],"resources":{"kinds":["task"]}}));
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
            &format!("/api/tasks/{task}/delegate"),
            json!({"node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}})
        )
        .await
        .0,
        403
    );
    assert_eq!(f.store.task(task).await.unwrap().status, "OPEN");
    assert!(f.store.runs().await.unwrap().is_empty());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM delegations")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        request(
            &app,
            &token,
            "POST",
            &format!("/api/tasks/{task}/claim"),
            json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
        )
        .await
        .0,
        200
    );
    cleanup(f, &url, &schema).await;
}
