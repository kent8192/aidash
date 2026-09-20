use aidash::{
    api,
    authorization::{
        Authorization,
        policy::{Evaluation, PolicyBundle},
    },
    config::Config,
    federation::Federation,
    registry::Registry,
    store::Store,
};
use axum::{Router, body::Body, http::Request};
use serde_json::{Value, json};
use sqlx::{
    Connection, Executor,
    postgres::{PgConnection, PgPoolOptions},
};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

async fn setup() -> (Router, Store, String, String) {
    let url = std::env::var("AIDASH_TEST_DATABASE_URL")
        .expect("AIDASH_TEST_DATABASE_URL must name a disposable PostgreSQL database");
    let schema = format!("authorization_{}", Uuid::new_v4().simple());
    let mut admin = PgConnection::connect(&url).await.unwrap();
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let search_path = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _| {
            let schema = search_path.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {schema}"))
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
        node_id: "aidash://authorization-test".into(),
    };
    let federation = Federation {
        store: store.clone(),
        registry: Registry::new(pool),
        config: Config {
            node_id: store.node_id.clone(),
            endpoint: "http://localhost:8080".into(),
            listen: "127.0.0.1:0".parse().unwrap(),
            database_url: url.clone(),
            nats_url: "nats://127.0.0.1:4222".into(),
            api_token: "authorization-test-token".into(),
            web_dir: "web/dist".into(),
            lease_seconds: 30,
        },
        client: reqwest::Client::new(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    (api::router(federation), store, url, schema)
}

async fn request(app: &Router, path: &str, body: Value, authenticated: bool) -> (u16, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if authenticated {
        request = request.header("authorization", "Bearer authorization-test-token");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn bundle() -> Value {
    json!({
        "tenant":"acme",
        "subjects":{
            "alice":{"kind":"user","roles":["editor"],"groups":["research"],"attributes":{"department":"research"}},
            "agent":{"kind":"agent","roles":["editor"],"attributes":{"department":"research"},"delegated_by":"alice"}
        },
        "groups":{"research":{"roles":["reader"]}},
        "roles":{"reader":{},"editor":{"inherits":["reader"]}},
        "policies":[{
            "id":"department-reader","effect":"allow","subjects":{"roles":["reader"]},
            "actions":["workspace.read"],"resources":{"kinds":["workspace"]},
            "condition":{"op":"eq","left":{"source":"subject","path":"/department"},
                "right":{"source":"resource","path":"/department"}}
        }]
    })
}

fn evaluation() -> Value {
    json!({"subject":"alice","action":"workspace.read","resource":{
        "tenant":"acme","kind":"workspace","id":"research-workspace","attributes":{"department":"research"}},
        "environment":{"network":"internal"}})
}

async fn get(app: &Router, path: &str) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header("authorization", "Bearer authorization-test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn cleanup(store: Store, url: &str, schema: &str) {
    store.pool.close().await;
    let mut admin = PgConnection::connect(url).await.unwrap();
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn authorization_api_enforces_policy_revision_revocation_and_audit() {
    let (app, store, url, schema) = setup().await;
    let mut policy = bundle();
    let update = json!({"expected_revision":0,"bundle":policy});
    assert_eq!(
        request(&app, "/api/authorization/acme", update.clone(), false)
            .await
            .0,
        401
    );
    let (status, created) = request(&app, "/api/authorization/acme", update.clone(), true).await;
    assert_eq!(status, 200, "policy creation: {created}");
    assert_eq!(created["revision"], 1);
    assert_eq!(
        request(&app, "/api/authorization/acme", update, true)
            .await
            .0,
        409
    );
    let path = "/api/authorization/acme/evaluate";
    let (status, allowed) = request(&app, path, evaluation(), true).await;
    assert_eq!(status, 200);
    assert_eq!(allowed["allowed"], true);
    assert_eq!(allowed["revision"], 1);
    assert_eq!(allowed["matched_policies"], json!(["department-reader"]));

    let mut foreign = evaluation();
    foreign["resource"]["tenant"] = json!("other");
    assert_eq!(request(&app, path, foreign, true).await.1["allowed"], false);
    let mut absent = evaluation();
    absent["resource"]["attributes"] = json!({});
    assert_eq!(request(&app, path, absent, true).await.1["allowed"], false);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM authorization_decisions")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        request(&app, "/api/authorization/acme/simulate", evaluation(), true)
            .await
            .1["allowed"],
        true
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM authorization_decisions")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, after, "dry-run must not add an audit record");

    policy["policies"].as_array_mut().unwrap().push(json!({
        "id":"suspended","effect":"deny","subjects":{"ids":["alice"]},
        "actions":["*"],"resources":{"kinds":["*"]}
    }));
    let (status, _) = request(
        &app,
        "/api/authorization/acme",
        json!({"expected_revision":1,"bundle":policy}),
        true,
    )
    .await;
    assert_eq!(status, 200);
    let denied = request(&app, path, evaluation(), true).await.1;
    assert_eq!(denied["allowed"], false);
    assert_eq!(denied["reason"], "explicit_deny");
    assert_eq!(denied["revision"], 2);
    let mut delegated = evaluation();
    delegated["subject"] = json!("agent");
    let denied = request(&app, path, delegated.clone(), true).await.1;
    assert_eq!(
        denied["allowed"], false,
        "agent must not exceed its delegator's authority"
    );
    assert_eq!(denied["reason"], "delegation_denied");

    policy["policies"].as_array_mut().unwrap().pop();
    policy["subjects"]["alice"]["enabled"] = json!(false);
    assert_eq!(
        request(
            &app,
            "/api/authorization/acme",
            json!({"expected_revision":2,"bundle":policy}),
            true
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(&app, path, delegated, true).await.1["allowed"],
        false
    );
    assert_eq!(
        request(&app, path, evaluation(), true).await.1["reason"],
        "subject_disabled"
    );
    let history: i64 =
        sqlx::query_scalar("SELECT count(*) FROM authorization_revisions WHERE tenant='acme'")
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(history, 3);
    let (status, snapshot) = get(&app, "/api/authorization/acme").await;
    assert_eq!(status, 200);
    assert_eq!(snapshot["revision"], 3);
    assert_eq!(snapshot["bundle"]["subjects"]["alice"]["enabled"], false);
    let (status, history) = get(&app, "/api/authorization/acme/revisions?after=1&limit=1").await;
    assert_eq!(status, 200);
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["revision"], 2);
    assert_eq!(history[0]["actor"], "operator");
    let (status, decisions) = get(&app, "/api/authorization/acme/decisions?limit=1").await;
    assert_eq!(status, 200);
    assert_eq!(decisions.as_array().unwrap().len(), 1);
    assert_eq!(decisions[0]["decision"]["allowed"], true);
    assert!(decisions[0].get("attributes").is_none());
    assert_eq!(
        get(&app, "/api/authorization/unknown/decisions").await.1,
        json!([])
    );
    cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn authorization_transaction_blocks_revocation_and_concurrent_updates_keep_history_consistent()
 {
    let (_app, store, url, schema) = setup().await;
    let service = Authorization {
        pool: store.pool.clone(),
    };
    let policy: PolicyBundle = serde_json::from_value(bundle()).unwrap();
    service
        .replace("acme", 0, policy.clone(), "operator")
        .await
        .unwrap();
    let input: Evaluation = serde_json::from_value(evaluation()).unwrap();
    let mut tx = store.pool.begin().await.unwrap();
    let decision = Authorization::evaluate_in_transaction(&mut tx, "acme", &input)
        .await
        .unwrap();
    assert!(decision.allowed);
    let mut concurrent = store.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *concurrent)
        .await
        .unwrap();
    let error =
        sqlx::query("UPDATE authorization_bundles SET revision=revision+1 WHERE tenant='acme'")
            .execute(&mut *concurrent)
            .await
            .unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("55P03")
    );
    concurrent.rollback().await.unwrap();
    tx.commit().await.unwrap();
    let mut revoked = policy.clone();
    revoked.subjects.get_mut("alice").unwrap().enabled = false;
    let (one, two) = tokio::join!(
        service.replace("acme", 1, revoked, "operator"),
        service.replace("acme", 1, policy, "operator")
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let error = one.err().or_else(|| two.err()).unwrap();
    assert!(matches!(error, aidash::Error::Conflict(_)));
    let snapshot = service.snapshot("acme").await.unwrap();
    let revisions = service.revisions("acme", 0, 100).await.unwrap();
    assert_eq!(snapshot.revision, 2);
    assert_eq!(revisions.len(), 2);
    assert_eq!(
        revisions[1]["document"],
        serde_json::to_value(snapshot.bundle).unwrap()
    );
    assert!(
        service
            .replace(
                "other",
                0,
                serde_json::from_value(bundle()).unwrap(),
                "operator"
            )
            .await
            .is_err()
    );
    assert!(service.revisions("other", 0, 100).await.unwrap().is_empty());
    cleanup(store, &url, &schema).await;
}
