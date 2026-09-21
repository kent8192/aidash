mod common;
use aidash::{
    api,
    domain::NewTask,
    registry::{Entry, Package},
};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

fn tool(id: &str) -> Entry {
    serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"tool","name":{"en":id},"description":{"en":"review regression"},"config":{"transport":"http","endpoint":"http://127.0.0.1:9/original","credential_env":null,"replay":"read_only"}})).unwrap()
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn installation_reconfiguration_keeps_manifest_and_events_idempotent() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let package = Package {
        entity: tool("installed"),
        author: "test".into(),
        permissions: vec![],
        dependencies: vec![],
    };
    let original = package.entity.clone();
    let (status, published) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/marketplace",
        json!(package),
    )
    .await;
    assert_eq!(status, 200, "{published}");
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/marketplace",
            json!(package)
        )
        .await
        .0,
        200
    );
    let path = "/api/marketplace/installed/1.0.0/install";
    for endpoint in [
        "http://127.0.0.1:9/first",
        "http://127.0.0.1:9/second",
        "http://127.0.0.1:9/second",
    ] {
        let (status, body) = request(
            &app,
            &f.config.api_token,
            "POST",
            path,
            json!({"digest":published["digest"],"config":{"endpoint":endpoint}}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            f.registry.get("installed", "1.0.0").await.unwrap().config["endpoint"],
            endpoint
        );
    }
    let (_, packages) = request(
        &app,
        &f.config.api_token,
        "GET",
        "/api/marketplace",
        Value::Null,
    )
    .await;
    assert_eq!(packages[0]["manifest"]["entity"], json!(original));
    let events = f.store.events(0, None, 1000).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "package.published")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "package.installed")
            .count(),
        2
    );
    let mut invalid = json!(package);
    invalid["dependecies"] = json!([]);
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/marketplace",
            invalid
        )
        .await
        .0,
        422
    );
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            path,
            json!({"digest":published["digest"],"configuration":{}})
        )
        .await
        .0,
        422
    );
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn ancestor_dependencies_and_invalid_local_executor_are_rejected() {
    let (f, url, schema) = setup().await;
    let workspace = f.store.create_workspace("tree", "goal").await.unwrap();
    let input = NewTask {
        title: "task".into(),
        description: "work".into(),
        requirements: json!({}),
        dependencies: vec![],
        parent_id: None,
    };
    let parent = f
        .store
        .create_task(workspace.id, &input, "operator", None)
        .await
        .unwrap();
    let mut child = input.clone();
    child.parent_id = Some(parent.id);
    child.dependencies = vec![parent.id];
    assert!(
        f.store
            .create_task(workspace.id, &child, "operator", None)
            .await
            .is_err()
    );
    child.dependencies.clear();
    let child = f
        .store
        .create_task(workspace.id, &child, "operator", None)
        .await
        .unwrap();
    let mut grandchild = input;
    grandchild.parent_id = Some(child.id);
    grandchild.dependencies = vec![parent.id];
    assert!(
        f.store
            .create_task(workspace.id, &grandchild, "operator", None)
            .await
            .is_err()
    );
    let app = api::router(f.clone());
    let mut entry = tool("bad-executor");
    for agent in [
        json!({"id":"","version":"1.0.0"}),
        json!({"id":"bad/path","version":"1.0.0"}),
        json!({"id":"missing","version":"latest"}),
        json!({"id":"missing","version":"1.0.0"}),
    ] {
        entry.config = json!({"transport":"agent","node_id":f.config.node_id,"agent":agent});
        assert_ne!(
            request(
                &app,
                &f.config.api_token,
                "POST",
                "/api/registry",
                json!(entry)
            )
            .await
            .0,
            200
        );
    }
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn visible_messages_and_events_survive_a_denied_burst() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
    let workspace = f.store.task(task).await.unwrap().workspace_id;
    f.store
        .message(workspace, "allowed", "older-visible", None)
        .await
        .unwrap();
    for _ in 0..110 {
        f.store
            .message(workspace, "denied", "private", None)
            .await
            .unwrap();
    }
    let denied: Vec<String> = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::Expr::cust("id::text"))
            .from(sea_orm::sea_query::Alias::new("messages"))
            .and_where(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("sender"))
                    .eq("denied"),
            )
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .fetch_all(&f.store.pool)
    .await
    .unwrap();
    policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-messages","effect":"deny","subjects":{"ids":["alice"]},"actions":["message.read"],"resources":{"kinds":["message"],"ids":denied}}));
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
    for path in [format!("/api/workspaces/{workspace}"), "/api/state".into()] {
        let (status, body) = request(&app, &token, "GET", &path, Value::Null).await;
        assert_eq!(status, 200, "{body}");
        assert!(
            body["events"].to_string().contains("older-visible"),
            "{body}"
        );
        if path.contains("workspaces") {
            assert!(body["messages"].to_string().contains("older-visible"));
        }
        assert!(!body.to_string().contains("private"));
    }
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn delegation_retry_and_run_message_have_one_durable_effect() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    bootstrap(&f, &app, "http://127.0.0.1:9").await;
    let workspace = f.store.create_workspace("operator", "goal").await.unwrap();
    let task = f
        .store
        .create_task(
            workspace.id,
            &NewTask {
                title: "work".into(),
                description: "work".into(),
                requirements: json!({}),
                dependencies: vec![],
                parent_id: None,
            },
            "human",
            None,
        )
        .await
        .unwrap();
    let path = format!("/api/tasks/{}/delegate", task.id);
    for _ in 0..2 {
        let (status, body) = request(
            &app,
            &f.config.api_token,
            "POST",
            &path,
            json!({"node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["delivered"], true);
    }
    assert_eq!(
        f.store
            .events(0, Some(workspace.id), 1000)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "task.delegated")
            .count(),
        1
    );
    let current = f.store.task(task.id).await.unwrap();
    let agent = f.registry.get("research", "1.0.0").await.unwrap();
    assert!(
        f.store
            .claim(
                task.id,
                current.revision,
                "aidash://other/agents/research@1.0.0",
                &agent
            )
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .claim(
                task.id,
                current.revision,
                &aidash::domain::qualified_agent(&f.config.node_id, "research", "1.0.0"),
                &agent
            )
            .await
            .unwrap()
            .status,
        "CLAIMED"
    );
    let run = f
        .store
        .runs()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.task_id == task.id)
        .unwrap();
    let key = Uuid::new_v4();
    for _ in 0..2 {
        assert_eq!(
            request(
                &app,
                &f.config.api_token,
                "POST",
                &format!("/api/runs/{}/message", run.id),
                json!({"content":"once","idempotency_key":key})
            )
            .await
            .0,
            200
        );
    }
    assert_eq!(
        f.store
            .snapshot(workspace.id)
            .await
            .unwrap()
            .messages
            .iter()
            .filter(|m| m.content == "once")
            .count(),
        1
    );
    cleanup(f, &url, &schema).await;
}

#[test]
fn remote_manifest_validates_secret_reference_without_resolving_it() {
    let mut entry = tool("remote-secret");
    entry.config["credential_env"] = json!("AIDASH_SECRET_REVIEW_REMOTE_ONLY_NOT_SET");
    let manifest: aidash::transactions::Manifest = serde_json::from_value(json!({
        "id":Uuid::new_v4(),"coordinator":"aidash://a","isolation":"serializable",
        "deadline":chrono::Utc::now()+chrono::Duration::minutes(1),
        "participants":[{"node_id":"aidash://a","mutations":[]},{"node_id":"aidash://b","mutations":[{"kind":"registry_register","entry":entry}]}]
    })).unwrap();
    manifest.validate().unwrap();
    assert!(
        aidash::registry::validate(&entry).is_err(),
        "the assigned node still resolves its local credential"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_run_details_keep_memory_home_namespace() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (_, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
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
    let run = f.store.runs().await.unwrap().remove(0);
    let mut remote = run.clone();
    remote.home_node = "aidash://another-home".into();
    f.store
        .remember(&remote, &json!({"secret":"remote-only"}))
        .await
        .unwrap();
    f.store
        .remember(&run, &json!({"local":"expected"}))
        .await
        .unwrap();
    let (status, details) = request(
        &app,
        &token,
        "GET",
        &format!("/api/runs/{}", run.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{details}");
    assert_eq!(details["memory"], json!({"local":"expected"}));
    cleanup(f, &url, &schema).await;
}

#[test]
fn agent_versions_fit_the_authorization_identity_limit() {
    let mut entry = tool(&"a".repeat(100));
    entry.kind = "agent".into();
    entry.config = json!({"model":{"id":"model","version":"1.0.0"},"instructions":"test"});
    entry.version = format!("1.0.0+{}", "x".repeat(33));
    aidash::registry::validate(&entry).unwrap();
    entry.version.push('x');
    assert!(aidash::registry::validate(&entry).is_err());
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn mesh_rejects_a_peer_substituting_another_node_identity() {
    use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
    let (f, url, schema) = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/federation/v0.1/observe", axum::routing::get(|| async {
            axum::Json(json!({"node_id":"aidash://substituted","runs":[],"human_requests":[],"invocations":[]}))
        }))).await.unwrap();
    });
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
                Expr::val("aidash://expected").into(),
                Expr::val(endpoint).into(),
                Expr::val("AIDASH_SECRET_TEST_PEER").into(),
                Expr::val("0.1").into(),
                Expr::val(true).into(),
            ])
            .to_string(PostgresQueryBuilder),
    )
    .execute(&f.store.pool)
    .await
    .unwrap();
    let (status, mesh) = request(
        &api::router(f.clone()),
        &f.config.api_token,
        "GET",
        "/api/mesh",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(mesh["nodes"], json!([]));
    assert_eq!(mesh["errors"][0]["node_id"], "aidash://expected");
    assert!(
        mesh["errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("identity mismatch")
    );
    server.abort();
    let _ = server.await;
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn oversized_agent_instructions_skills_and_tools_are_rejected_at_registration() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    bootstrap(&f, &app, "http://127.0.0.1:9").await;
    let mut skill = tool("large-skill");
    skill.kind = "skill".into();
    skill.config = json!({"instructions":"x".repeat(128000)});
    f.registry.register(skill).await.unwrap();
    let mut large_tool = tool("large-schema");
    large_tool.schema = json!({"type":"object","description":"x".repeat(128000)});
    f.registry.register(large_tool).await.unwrap();
    for source in ["instructions", "skills", "tools"] {
        let mut agent = f.registry.get("research", "1.0.0").await.unwrap();
        agent.id = format!("oversized-{source}");
        agent.config[source] = match source {
            "instructions" => json!("x".repeat(128000)),
            "skills" => json!([{"id":"large-skill","version":"1.0.0"}]),
            _ => json!([{"id":"large-schema","version":"1.0.0"}]),
        };
        assert!(f.registry.register(agent.clone()).await.is_err());
        assert_eq!(
            request(
                &app,
                &f.config.api_token,
                "POST",
                "/api/registry",
                json!(agent)
            )
            .await
            .0,
            400
        );
        assert!(f.registry.get(&agent.id, &agent.version).await.is_err());
    }
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and NATS"]
async fn malformed_broker_messages_do_not_stop_valid_delivery() {
    use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
    let (f, url, schema) = setup().await;
    let bus = aidash::bus::EventBus::connect(
        &std::env::var("AIDASH_TEST_NATS_URL").unwrap(),
        &format!("aidash://review-{}", Uuid::new_v4().simple()),
    )
    .await
    .unwrap();
    let id = Uuid::new_v4();
    for payload in [
        "not-json".to_owned(),
        "{}".to_owned(),
        "{\"id\":\"invalid\"}".to_owned(),
        json!({"id":id}).to_string(),
    ] {
        bus.context
            .publish(bus.subject.clone(), payload.into())
            .await
            .unwrap()
            .await
            .unwrap();
    }
    let worker_bus = bus.clone();
    let worker_f = f.clone();
    let consumer = tokio::spawn(async move { worker_bus.consumer(worker_f).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                &Query::select()
                    .expr(Expr::col(sea_orm::sea_query::Asterisk).count())
                    .from(Alias::new("inbox"))
                    .and_where(Expr::col(Alias::new("event_id")).eq(Expr::cust("$1")))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(id)
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
            if count == 1 {
                break;
            }
            assert!(
                !consumer.is_finished(),
                "consumer exited on malformed input"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(!consumer.is_finished());
    consumer.abort();
    let _ = consumer.await;
    bus.context.delete_stream(&bus.stream_name).await.unwrap();
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn registry_replays_emit_once_and_disabled_peers_can_lose_trust() {
    use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    for _ in 0..2 {
        let (status, body) = request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/registry",
            json!(tool("replayed")),
        )
        .await;
        assert_eq!(status, 200, "{body}");
    }
    assert_eq!(
        f.store
            .events(0, None, 1000)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "registry.registered")
            .count(),
        1
    );
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
                Expr::val("aidash://disabled").into(),
                Expr::val("http://127.0.0.1:9").into(),
                Expr::val("AIDASH_SECRET_TEST_PEER").into(),
                Expr::val("0.1").into(),
                Expr::val(false).into(),
            ])
            .to_string(PostgresQueryBuilder),
    )
    .execute(&f.store.pool)
    .await
    .unwrap();
    sqlx::query(
        &Query::insert()
            .into_table(Alias::new("atomic_peer_trust"))
            .columns(["node_id", "enabled"].map(Alias::new))
            .values_panic([
                Expr::val("aidash://disabled").into(),
                Expr::val(true).into(),
            ])
            .to_string(PostgresQueryBuilder),
    )
    .execute(&f.store.control_pool)
    .await
    .unwrap();
    let (status, body) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/transactions/trust",
        json!({"node_id":"aidash://disabled", "enabled":false}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let enabled: bool = sqlx::query_scalar(
        &Query::select()
            .column(Alias::new("enabled"))
            .from(Alias::new("atomic_peer_trust"))
            .and_where(Expr::col(Alias::new("node_id")).eq("aidash://disabled"))
            .to_string(PostgresQueryBuilder),
    )
    .fetch_one(&f.store.control_pool)
    .await
    .unwrap();
    assert!(!enabled);
    assert_ne!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/transactions/trust",
            json!({"node_id":"aidash://disabled", "enabled":true})
        )
        .await
        .0,
        200
    );
    cleanup(f, &url, &schema).await;
}
