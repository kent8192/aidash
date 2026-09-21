mod common;
use aidash::{api, domain::qualified_agent, store::Store};
use axum::{Router, body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn peer(app: &Router, node: &str, path: &str, input: Value) -> (u16, Value) {
    let token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("x-aidash-node", node)
                .header("x-aidash-protocol", "0.1")
                .header("content-type", "application/json")
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn receiver_admission_is_idempotent_scoped_and_revalidated_after_reconnect() {
    let (mut a, au, aschema) = setup().await;
    let (mut b, bu, bschema) = setup().await;
    b.config.node_id = "aidash://admission-host".into();
    b.store.node_id = b.config.node_id.clone();
    let al = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    a.config.endpoint = format!("http://{}", al.local_addr().unwrap());
    b.config.endpoint = format!("http://{}", bl.local_addr().unwrap());
    let aa = api::router(a.clone());
    let ba = api::router(b.clone());
    let (mut ap, at, task) = bootstrap(&a, &aa, "http://localhost:1").await;
    let (bp, bt, _) = bootstrap(&b, &ba, "http://localhost:1").await;
    ap["subjects"][qualified_agent(&b.config.node_id, "research", "1.0.0")] =
        json!({"kind":"agent"});
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":1,"bundle":ap})
        )
        .await
        .0,
        200
    );
    for (local, other) in [(&a, &b), (&b, &a)] {
        sqlx::query(
            &sea_orm::sea_query::Query::insert()
                .into_table(sea_orm::sea_query::Alias::new("peers"))
                .columns([
                    sea_orm::sea_query::Alias::new("node_id"),
                    sea_orm::sea_query::Alias::new("endpoint"),
                    sea_orm::sea_query::Alias::new("credential_env"),
                    sea_orm::sea_query::Alias::new("protocol_version"),
                    sea_orm::sea_query::Alias::new("enabled"),
                ])
                .values_panic([
                    sea_orm::sea_query::Expr::cust("$1"),
                    sea_orm::sea_query::Expr::cust("$2"),
                    sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
                    sea_orm::sea_query::Expr::cust("'0.1'"),
                    sea_orm::sea_query::Expr::cust("TRUE"),
                ])
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
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
    let mapping = json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":credential["credential"]["id"],"enabled":true,"expected_revision":0});
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            "/api/authorization/acme/peer-mappings",
            mapping.clone()
        )
        .await
        .0,
        200
    );
    let aapp = aa.clone();
    let bapp = ba.clone();
    let aserver = tokio::spawn(async move { axum::serve(al, aapp).await.unwrap() });
    let bserver = tokio::spawn(async move { axum::serve(bl, bapp).await.unwrap() });
    let grant = Uuid::new_v4();
    let input = json!({"id":grant,"node_id":b.config.node_id,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300});
    let path = format!("/api/tasks/{task}/remote-grants");
    let (status, prepared) = request(&aa, &at, "POST", &path, input.clone()).await;
    assert_eq!(status, 200, "{prepared}");
    let admit = "/federation/v0.1/scoped/execution/admissions";
    let (first, second) = tokio::join!(
        peer(&ba, &a.config.node_id, admit, json!({"grant_id":grant})),
        peer(&ba, &a.config.node_id, admit, json!({"grant_id":grant}))
    );
    assert_eq!(first.0, 200, "{}", first.1);
    assert_eq!(
        first, second,
        "duplicate delivery must return the same local binding"
    );
    let source_task = a.store.task(task).await.unwrap();
    assert_eq!(
        peer(
            &ba,
            &a.config.node_id,
            "/federation/v0.1/offers",
            json!({"task":source_task,"agent":{"id":"research","version":"1.0.0"}})
        )
        .await
        .0,
        403,
        "legacy offers cannot bypass the accepted scoped binding"
    );
    let receipt = first.1;
    assert_eq!(receipt["task_id"], task.to_string());
    let verification = format!("{admit}/{}/verify", receipt["id"].as_str().unwrap());
    assert_eq!(
        peer(&ba, &a.config.node_id, &verification, json!({})).await,
        (200, json!(true))
    );
    let mut fresh = b.clone();
    fresh.store = Store::from_pool(
        b.store
            .pool
            .options()
            .clone()
            .connect_with(b.store.pool.connect_options().as_ref().clone())
            .await
            .unwrap(),
        b.config.node_id.clone(),
    )
    .await
    .unwrap();
    let fresh_app = api::router(fresh.clone());
    assert_eq!(
        peer(
            &fresh_app,
            &a.config.node_id,
            admit,
            json!({"grant_id":grant})
        )
        .await,
        (200, receipt.clone())
    );
    let replacement = Uuid::new_v4();
    let mut another = input.clone();
    another["id"] = json!(replacement);
    assert_eq!(request(&aa, &at, "POST", &path, another).await.0, 200);
    assert_eq!(
        peer(
            &fresh_app,
            &a.config.node_id,
            admit,
            json!({"grant_id":replacement})
        )
        .await
        .0,
        409,
        "another grant cannot replace the task's existing admission"
    );
    // Receiver policy scopes a foreign workspace by its qualified source ID.
    let mut denied = bp.clone();
    denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-foreign-workspace","effect":"deny","subjects":{"any":true},"actions":["workspace.read"],"resources":{"kinds":["workspace"]},"condition":{"op":"eq","left":{"source":"resource","path":"/source_node"},"right":{"source":"literal","value":a.config.node_id}}}));
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":1,"bundle":denied})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        403
    );
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":2,"bundle":bp})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        200
    );
    let mut denied_model = bp.clone();
    denied_model["policies"].as_array_mut().unwrap().push(json!({"id":"deny-model-in-workspace","effect":"deny","subjects":{"any":true},"actions":["model.infer"],"resources":{"kinds":["model"]},"condition":{"op":"eq","left":{"source":"resource","path":"/source_workspace_id"},"right":{"source":"literal","value":source_task.workspace_id}}}));
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":3,"bundle":denied_model})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        403,
        "executor metadata checks must include the admitted workspace context"
    );
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":4,"bundle":bp})
        )
        .await
        .0,
        200
    );
    let mut denied = ap.clone();
    denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-source-task","effect":"deny","subjects":{"any":true},"actions":["task.read"],"resources":{"kinds":["task"]}}));
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":2,"bundle":denied})
        )
        .await
        .0,
        200
    );
    let denied = peer(&fresh_app, &a.config.node_id, &verification, json!({})).await;
    assert_ne!(denied.0, 200);
    assert!(!denied.1.to_string().contains("Research"));
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":3,"bundle":ap})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        200
    );
    // Source outage cannot be replaced with the previously accepted grant.
    sqlx::query(
        &sea_orm::sea_query::Query::update()
            .table(sea_orm::sea_query::Alias::new("peers"))
            .value(
                sea_orm::sea_query::Alias::new("endpoint"),
                sea_orm::sea_query::Expr::cust("'http://127.0.0.1:1'"),
            )
            .and_where(sea_orm::sea_query::Expr::cust("node_id = $1"))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&a.config.node_id)
    .execute(&b.store.pool)
    .await
    .unwrap();
    assert_ne!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        200
    );
    sqlx::query(
        &sea_orm::sea_query::Query::update()
            .table(sea_orm::sea_query::Alias::new("peers"))
            .value(
                sea_orm::sea_query::Alias::new("endpoint"),
                sea_orm::sea_query::Expr::cust("$2"),
            )
            .and_where(sea_orm::sea_query::Expr::cust("node_id = $1"))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&a.config.node_id)
    .bind(&a.config.endpoint)
    .execute(&b.store.pool)
    .await
    .unwrap();
    assert_eq!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        200
    );
    let (_, new_credential) = request(
        &ba,
        &b.config.api_token,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    let mut rotated = mapping.clone();
    rotated["expected_revision"] = json!(1);
    rotated["credential_id"] = new_credential["credential"]["id"].clone();
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            "/api/authorization/acme/peer-mappings",
            rotated
        )
        .await
        .0,
        200
    );
    assert_ne!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        200
    );
    let mut restored = mapping;
    restored["expected_revision"] = json!(2);
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            "/api/authorization/acme/peer-mappings",
            restored
        )
        .await
        .0,
        200
    );
    assert_eq!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        200
    );
    let (status, state) = request(&ba, &bt, "GET", "/api/state", json!({})).await;
    assert_eq!(status, 200);
    assert!(
        !state.to_string().contains(&task.to_string()),
        "receiver tenants must not receive unscoped source task events"
    );
    assert_eq!(
        request(
            &aa,
            &at,
            "POST",
            &format!("{path}/{grant}/revoke"),
            json!({})
        )
        .await
        .0,
        200
    );
    assert_ne!(
        peer(&fresh_app, &a.config.node_id, &verification, json!({}))
            .await
            .0,
        200
    );
    assert_ne!(
        peer(
            &fresh_app,
            &a.config.node_id,
            admit,
            json!({"grant_id":grant})
        )
        .await
        .0,
        200
    );
    let count: i64 = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
            .from(sea_orm::sea_query::Alias::new(
                "authorization_remote_admissions",
            ))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .fetch_one(&b.store.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    // Race scoped admission against legacy run creation for a fresh task. The
    // two modes must never both commit, regardless of which request wins.
    let (status, candidate) = request(
        &aa,
        &at,
        "POST",
        &format!("/api/workspaces/{}/tasks", source_task.workspace_id),
        json!({"title":"Admission race","description":"Fixture"}),
    )
    .await;
    assert_eq!(status, 200);
    let candidate: aidash::domain::Task = serde_json::from_value(candidate).unwrap();
    let race_grant = Uuid::new_v4();
    let mut race_input = input.clone();
    race_input["id"] = json!(race_grant);
    assert_eq!(
        request(
            &aa,
            &at,
            "POST",
            &format!("/api/tasks/{}/remote-grants", candidate.id),
            race_input
        )
        .await
        .0,
        200
    );
    let (scoped, legacy) = tokio::join!(
        peer(
            &fresh_app,
            &a.config.node_id,
            admit,
            json!({"grant_id":race_grant})
        ),
        b.store
            .accept_run(&candidate, &a.config.node_id, "research", "1.0.0")
    );
    let runs: i64 = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
            .from(sea_orm::sea_query::Alias::new("runs"))
            .and_where(sea_orm::sea_query::Expr::cust(
                "home_node = $1 AND task_id = $2",
            ))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&a.config.node_id)
    .bind(candidate.id)
    .fetch_one(&b.store.pool)
    .await
    .unwrap();
    let admissions: i64 = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
            .from(sea_orm::sea_query::Alias::new(
                "authorization_remote_admissions",
            ))
            .and_where(sea_orm::sea_query::Expr::cust(
                "source_node = $1 AND task_id = $2",
            ))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&a.config.node_id)
    .bind(candidate.id)
    .fetch_one(&b.store.pool)
    .await
    .unwrap();
    assert_eq!(
        runs + admissions,
        1,
        "legacy and scoped admission must be mutually exclusive"
    );
    match legacy {
        Ok(run) => {
            assert_eq!(scoped.0, 409);
            sqlx::query(
                &sea_orm::sea_query::Query::delete()
                    .from_table(sea_orm::sea_query::Alias::new("runs"))
                    .and_where(sea_orm::sea_query::Expr::cust("id = $1"))
                    .to_string(sea_orm::sea_query::PostgresQueryBuilder),
            )
            .bind(run.id)
            .execute(&b.store.pool)
            .await
            .unwrap();
        }
        Err(aidash::Error::Forbidden) => assert_eq!(scoped.0, 200),
        Err(error) => panic!("unexpected legacy admission error: {error}"),
    }
    for f in [&a, &b] {
        let count: i64 = sqlx::query_scalar(
            &sea_orm::sea_query::Query::select()
                .expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
                .from(sea_orm::sea_query::Alias::new("runs"))
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
        assert_eq!(
            count, 0,
            "admission may not bypass scoped worker activation"
        );
    }
    drop(fresh_app);
    fresh.store.pool.close().await;
    fresh.store.control_pool.close().await;
    aserver.abort();
    bserver.abort();
    let _ = aserver.await;
    let _ = bserver.await;
    cleanup(a, &au, &aschema).await;
    cleanup(b, &bu, &bschema).await;
}
