mod common;
use aidash::api;
use axum::{Router, body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use tower::ServiceExt;

const SOURCE: &str = "aidash://source";
async fn discover(
    app: &Router,
    node: &str,
    token: &str,
    tenant: &str,
    subject: &str,
) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/federation/v0.1/scoped/discover")
                .header("authorization", format!("Bearer {token}"))
                .header("x-aidash-node", node)
                .header("x-aidash-protocol", "0.1")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"tenant":tenant,"subject":subject,"search":{}}).to_string(),
                ))
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn inbound_discovery_requires_exact_mapping_and_current_local_authority() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (mut policy, subject_token, _) = bootstrap(&f, &app, "http://localhost:1").await;
    // A peer record is a fixture prerequisite, not a substitute for the inbound
    // middleware: every discovery below passes through real authentication.
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
                sea_orm::sea_query::Expr::cust("'http://localhost:1'"),
                sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
                sea_orm::sea_query::Expr::cust("'0.1'"),
                sea_orm::sea_query::Expr::cust("TRUE"),
            ])
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(SOURCE)
    .execute(&f.store.pool)
    .await
    .unwrap();
    let peer_token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
    assert_eq!(
        discover(&app, SOURCE, &peer_token, "remote", "bob").await.0,
        403
    );
    let (_, issued) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    let credential = &issued["credential"]["id"];
    let mut mapping = json!({"source_node":SOURCE,"source_tenant":"remote","source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0});
    let path = "/api/authorization/acme/peer-mappings";
    assert_eq!(
        request(&app, &subject_token, "POST", path, mapping.clone())
            .await
            .0,
        403
    );
    let (status, binding) = request(&app, &f.config.api_token, "POST", path, mapping.clone()).await;
    assert_eq!(status, 200, "{binding}");
    assert_eq!(binding["revision"], 1);
    assert_eq!(
        request(&app, &f.config.api_token, "POST", path, mapping.clone())
            .await
            .0,
        409
    );
    for (node, token, tenant, subject, expected) in [
        (SOURCE, peer_token.as_str(), "remote", "bob", 200),
        (SOURCE, peer_token.as_str(), "other", "bob", 403),
        (SOURCE, peer_token.as_str(), "remote", "alice", 403),
        ("aidash://forged", peer_token.as_str(), "remote", "bob", 401),
        (SOURCE, subject_token.as_str(), "remote", "bob", 401),
        (SOURCE, f.config.api_token.as_str(), "remote", "bob", 401),
    ] {
        let (status, body) = discover(&app, node, token, tenant, subject).await;
        assert_eq!(status, expected, "{node} {tenant}/{subject}: {body}");
        if status == 200 {
            assert_eq!(body.as_array().unwrap().len(), 1);
            assert_eq!(body[0]["id"], "research");
        }
    }
    mapping["expected_revision"] = json!(1);
    mapping["enabled"] = json!(false);
    assert_eq!(
        request(&app, &f.config.api_token, "POST", path, mapping.clone())
            .await
            .0,
        200
    );
    assert_eq!(
        discover(&app, SOURCE, &peer_token, "remote", "bob").await.0,
        403
    );
    mapping["expected_revision"] = json!(2);
    mapping["enabled"] = json!(true);
    assert_eq!(
        request(&app, &f.config.api_token, "POST", path, mapping.clone())
            .await
            .0,
        200
    );
    // Tenant approval is independent of the mapped subject's wildcard policy.
    assert_eq!(request(&app, &f.config.api_token, "POST", "/api/authorization/acme/catalog", json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false})).await.0, 200);
    assert_eq!(
        discover(&app, SOURCE, &peer_token, "remote", "bob").await.1,
        json!([])
    );
    assert_eq!(request(&app, &f.config.api_token, "POST", "/api/authorization/acme/catalog", json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":2,"enabled":true})).await.0, 200);
    // An explicit deny on the mapped local identity filters the remote result.
    policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-agent","effect":"deny","subjects":{"any":true},"actions":["agent.execute"],"resources":{"kinds":["agent"]}}));
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
    let (status, entries) = discover(&app, SOURCE, &peer_token, "remote", "bob").await;
    assert_eq!(status, 200);
    assert_eq!(entries, json!([]));
    // Credential revocation invalidates even an enabled, previously used map.
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            &format!(
                "/api/authorization/acme/credentials/{}/revoke",
                credential.as_str().unwrap()
            ),
            json!({})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        discover(&app, SOURCE, &peer_token, "remote", "bob").await.0,
        403
    );
    mapping["expected_revision"] = json!(3);
    assert_eq!(
        request(&app, &f.config.api_token, "POST", path, mapping.clone())
            .await
            .0,
        403
    );
    mapping["enabled"] = json!(false);
    assert_eq!(
        request(&app, &f.config.api_token, "POST", path, mapping)
            .await
            .0,
        200
    );
    let count: i64 = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
            .from(sea_orm::sea_query::Alias::new(
                "authorization_peer_mapping_history",
            ))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(count, 4, "failed writes must not create history");
    let (status, first) = request(
        &app,
        &f.config.api_token,
        "GET",
        "/api/authorization/acme/peer-mapping-history?limit=2",
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(first.as_array().unwrap().len(), 2);
    assert_eq!(first[0]["revision"], 1);
    let (_, second) = request(
        &app,
        &f.config.api_token,
        "GET",
        &format!(
            "/api/authorization/acme/peer-mapping-history?after={}&limit=2",
            first[1]["sequence"]
        ),
        json!({}),
    )
    .await;
    assert_eq!(second.as_array().unwrap().len(), 2);
    assert_eq!(second[0]["revision"], 3);
    assert_eq!(
        request(
            &app,
            &subject_token,
            "GET",
            "/api/authorization/acme/peer-mapping-history",
            json!({})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "GET",
            "/api/authorization/acme/peer-mapping-history?limit=201",
            json!({})
        )
        .await
        .0,
        400
    );
    cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn mappings_cannot_cross_tenants_and_expiry_or_disabled_subject_denies_discovery() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (mut policy, _, _) = bootstrap(&f, &app, "http://localhost:1").await;
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
                sea_orm::sea_query::Expr::cust("'http://localhost:1'"),
                sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
                sea_orm::sea_query::Expr::cust("'0.1'"),
                sea_orm::sea_query::Expr::cust("TRUE"),
            ])
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(SOURCE)
    .execute(&f.store.pool)
    .await
    .unwrap();
    let peer_token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
    let (_, issued) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    let credential = issued["credential"]["id"].clone();
    let mapping = json!({"source_node":SOURCE,"source_tenant":"remote","source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0});
    let mut other_policy = policy.clone();
    other_policy["tenant"] = json!("other");
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/authorization/other",
            json!({"expected_revision":0,"bundle":other_policy})
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
            "/api/authorization/other/peer-mappings",
            mapping.clone()
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/authorization/acme/peer-mappings",
            mapping.clone()
        )
        .await
        .0,
        200
    );
    let (_, other_issued) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/authorization/other/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    let mut other_mapping = mapping.clone();
    other_mapping["credential_id"] = other_issued["credential"]["id"].clone();
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/authorization/other/peer-mappings",
            other_mapping.clone()
        )
        .await
        .0,
        409
    );
    other_mapping["expected_revision"] = json!(1);
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/authorization/other/peer-mappings",
            other_mapping
        )
        .await
        .0,
        409
    );
    policy["subjects"]["alice"]["enabled"] = json!(false);
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
        discover(&app, SOURCE, &peer_token, "remote", "bob").await.0,
        403
    );
    policy["subjects"]["alice"]["enabled"] = json!(true);
    policy["policies"][0]["condition"] = json!({"op":"eq","left":{"source":"environment","path":"/transport"},"right":{"source":"literal","value":"federation"}});
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
        discover(&app, SOURCE, &peer_token, "remote", "bob").await.0,
        200
    );
    // Expiry is a durable database clock check, including for active mappings.
    sqlx::query(
        &sea_orm::sea_query::Query::update()
            .table(sea_orm::sea_query::Alias::new("authorization_credentials"))
            .value(
                sea_orm::sea_query::Alias::new("created_at"),
                sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '2 SECONDS'"),
            )
            .value(
                sea_orm::sea_query::Alias::new("expires_at"),
                sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
            )
            .and_where(sea_orm::sea_query::Expr::cust("id = $1"))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(uuid::Uuid::parse_str(credential.as_str().unwrap()).unwrap())
    .execute(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(
        discover(&app, SOURCE, &peer_token, "remote", "bob").await.0,
        403
    );
    cleanup(f, &url, &schema).await;
}
