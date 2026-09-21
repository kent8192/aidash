mod common;
use aidash::{api, domain::qualified_agent, federation::Peer};
use axum::{Router, body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
async fn verify(app: &Router, node: &str, grant: Uuid) -> (u16, Value) {
    let token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/federation/v0.1/scoped/execution/grants/verify")
                .header("authorization", format!("Bearer {token}"))
                .header("x-aidash-node", node)
                .header("x-aidash-protocol", "0.1")
                .header("content-type", "application/json")
                .body(Body::from(json!({"grant_id":grant}).to_string()))
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
async fn durable_grants_bind_both_nodes_and_revalidate_after_restarts_and_revocation() {
    let (a, au, aschema) = setup().await;
    let (mut b, bu, bschema) = setup().await;
    b.config.node_id = "aidash://grant-host".into();
    b.store.node_id = b.config.node_id.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    b.config.endpoint = format!("http://{}", listener.local_addr().unwrap());
    let aa = api::router(a.clone());
    let ba = api::router(b.clone());
    let (mut policy, token, task) = bootstrap(&a, &aa, "http://localhost:1").await;
    bootstrap(&b, &ba, "http://localhost:1").await;
    let executor = qualified_agent(&b.config.node_id, "research", "1.0.0");
    policy["subjects"][&executor] = json!({"kind":"agent"});
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":1,"bundle":policy})
        )
        .await
        .0,
        200
    );
    sqlx::query("INSERT INTO peers(node_id,endpoint,credential_env,protocol_version,enabled) VALUES($1,'http://localhost:1','AIDASH_SECRET_TEST_PEER','0.1',true)").bind(&a.config.node_id).execute(&b.store.pool).await.unwrap();
    let peer_app = ba.clone();
    let server = tokio::spawn(async move { axum::serve(listener, peer_app).await.unwrap() });
    a.register_peer(Peer {
        node_id: b.config.node_id.clone(),
        endpoint: b.config.endpoint.clone(),
        credential_env: "AIDASH_SECRET_TEST_PEER".into(),
        protocol_version: "0.1".into(),
        enabled: true,
    })
    .await
    .unwrap();
    let (_, issued) = request(
        &ba,
        &b.config.api_token,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    assert_eq!(request(&ba,&b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true})).await.0,200);
    let path = format!("/api/tasks/{task}/remote-grants");
    let id = Uuid::new_v4();
    let input = json!({"id":id,"node_id":b.config.node_id,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300});
    assert_eq!(
        request(&aa, &a.config.api_token, "POST", &path, input.clone())
            .await
            .0,
        403
    );
    let (status, prepared) = request(&aa, &token, "POST", &path, input.clone()).await;
    assert_eq!(status, 200, "{prepared}");
    assert_eq!(
        request(&aa, &token, "POST", &path, input.clone()).await,
        (200, prepared.clone()),
        "retry must preserve id and expiry"
    );
    assert_eq!(verify(&aa, &b.config.node_id, id).await, (200, json!(true)));
    // A newly constructed store/router must use the persisted grant, not memory.
    let mut fresh = a.clone();
    fresh.store = aidash::store::Store::from_pool(
        a.store
            .pool
            .options()
            .clone()
            .connect_with(a.store.pool.connect_options().as_ref().clone())
            .await
            .unwrap(),
        a.config.node_id.clone(),
    )
    .await
    .unwrap();
    let fresh_app = api::router(fresh.clone());
    assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 200);
    let mut different = input.clone();
    different["agent"]["id"] = json!("nonexistent");
    assert_ne!(request(&aa, &token, "POST", &path, different).await.0, 200);
    // Receiver model metadata changes invalidate the exact execution snapshot,
    // even when the Agent's public definition remains unchanged.
    let metadata: Value = sqlx::query_scalar("SELECT metadata FROM registry WHERE id='model'")
        .fetch_one(&b.store.pool)
        .await
        .unwrap();
    let mut changed = metadata.clone();
    changed["description"]["en"] = json!("changed fixture");
    sqlx::query("UPDATE registry SET metadata=$1 WHERE id='model'")
        .bind(changed)
        .execute(&b.store.pool)
        .await
        .unwrap();
    assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 403);
    sqlx::query("UPDATE registry SET metadata=$1 WHERE id='model'")
        .bind(metadata)
        .execute(&b.store.pool)
        .await
        .unwrap();
    assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 200);
    let mut denied = policy.clone();
    denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-remote-model","effect":"deny","subjects":{"ids":[executor]},"actions":["model.infer"],"resources":{"kinds":["model"]},"condition":{"op":"eq","left":{"source":"resource","path":"/config/provider"},"right":{"source":"literal","value":"openai"}}}));
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
    assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 403);
    assert_eq!(request(&aa,&token,"POST",&path,json!({"id":Uuid::new_v4(),"node_id":b.config.node_id,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300})).await.0,403);
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":3,"bundle":policy})
        )
        .await
        .0,
        200
    );
    assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 200);
    let mut denied_read = policy.clone();
    denied_read["policies"].as_array_mut().unwrap().push(json!({"id":"hide-task","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[task.to_string()]}}));
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":4,"bundle":denied_read})
        )
        .await
        .0,
        200
    );
    let (status, state) = request(&aa, &token, "GET", "/api/state", json!({})).await;
    assert_eq!(status, 200);
    assert!(
        !state.to_string().contains(&id.to_string()),
        "grant audit events must respect task visibility"
    );
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            "/api/authorization/acme",
            json!({"expected_revision":5,"bundle":policy})
        )
        .await
        .0,
        200
    );
    // The fixture has one peer secret. Disable its original binding while
    // testing an authenticated different node, then restore it.
    sqlx::query("UPDATE peers SET enabled=false WHERE node_id=$1")
        .bind(&b.config.node_id)
        .execute(&a.store.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO peers(node_id,endpoint,credential_env,protocol_version,enabled) VALUES('aidash://unrelated','http://localhost:1','AIDASH_SECRET_TEST_PEER','0.1',true)").execute(&a.store.pool).await.unwrap();
    assert_eq!(
        verify(&aa, "aidash://unrelated", id).await.0,
        403,
        "an authenticated unrelated peer must not use the grant"
    );
    sqlx::query("DELETE FROM peers WHERE node_id='aidash://unrelated'")
        .execute(&a.store.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE peers SET enabled=true WHERE node_id=$1")
        .bind(&b.config.node_id)
        .execute(&a.store.pool)
        .await
        .unwrap();
    let revoke = format!("{path}/{id}/revoke");
    let (status, revoked) = request(&aa, &token, "POST", &revoke, json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(revoked["revoked"], true);
    assert_eq!(
        request(&aa, &token, "POST", &revoke, json!({})).await,
        (200, revoked)
    );
    assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 403);
    assert_eq!(
        request(&aa, &token, "POST", &path, input.clone()).await.0,
        409
    );
    let expiry = Uuid::new_v4();
    let mut next = input.clone();
    next["id"] = json!(expiry);
    assert_eq!(request(&aa, &token, "POST", &path, next).await.0, 200);
    sqlx::query("UPDATE authorization_remote_grants SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(expiry).execute(&a.store.pool).await.unwrap();
    assert_eq!(verify(&fresh_app, &b.config.node_id, expiry).await.0, 403);
    let current = Uuid::new_v4();
    let mut next = input.clone();
    next["id"] = json!(current);
    assert_eq!(request(&aa, &token, "POST", &path, next).await.0, 200);
    sqlx::query("UPDATE tasks SET revision=revision+1 WHERE id=$1")
        .bind(task)
        .execute(&a.store.pool)
        .await
        .unwrap();
    assert_eq!(verify(&fresh_app, &b.config.node_id, current).await.0, 403);
    sqlx::query("UPDATE tasks SET revision=revision-1 WHERE id=$1")
        .bind(task)
        .execute(&a.store.pool)
        .await
        .unwrap();
    assert_eq!(verify(&fresh_app, &b.config.node_id, current).await.0, 200);
    let (_, replacement) = request(
        &aa,
        &a.config.api_token,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    let replacement_token = replacement["token"].as_str().unwrap();
    let mut same = input.clone();
    same["id"] = json!(current);
    assert_eq!(
        request(&aa, replacement_token, "POST", &path, same).await.0,
        409,
        "a replacement credential cannot take over an existing grant"
    );
    let source_credential: Uuid =
        sqlx::query_scalar("SELECT credential_id FROM authorization_remote_grants WHERE id=$1")
            .bind(current)
            .fetch_one(&a.store.pool)
            .await
            .unwrap();
    assert_eq!(
        request(
            &aa,
            &a.config.api_token,
            "POST",
            &format!("/api/authorization/acme/credentials/{source_credential}/revoke"),
            json!({})
        )
        .await
        .0,
        200
    );
    assert_eq!(verify(&fresh_app, &b.config.node_id, current).await.0, 403);
    let replacement_id = Uuid::new_v4();
    let mut next = input.clone();
    next["id"] = json!(replacement_id);
    assert_eq!(
        request(&aa, replacement_token, "POST", &path, next).await.0,
        200
    );
    assert_eq!(
        verify(&fresh_app, &b.config.node_id, replacement_id)
            .await
            .0,
        200
    );
    let (_, receiver_replacement) = request(
        &ba,
        &b.config.api_token,
        "POST",
        "/api/authorization/acme/credentials",
        json!({"subject":"alice"}),
    )
    .await;
    for (revision, credential, expected) in [
        (1, &receiver_replacement["credential"]["id"], 403),
        (2, &issued["credential"]["id"], 200),
    ] {
        assert_eq!(request(&ba,&b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":credential,"expected_revision":revision,"enabled":true})).await.0,200);
        assert_eq!(
            verify(&fresh_app, &b.config.node_id, replacement_id)
                .await
                .0,
            expected,
            "receiver credential rebinding must invalidate the prepared authority"
        );
    }
    assert_eq!(
        request(
            &ba,
            &b.config.api_token,
            "POST",
            &format!(
                "/api/authorization/acme/credentials/{}/revoke",
                issued["credential"]["id"].as_str().unwrap()
            ),
            json!({})
        )
        .await
        .0,
        200
    );
    assert_ne!(
        verify(&fresh_app, &b.config.node_id, replacement_id)
            .await
            .0,
        200
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM authorization_remote_grants")
        .fetch_one(&a.store.pool)
        .await
        .unwrap();
    assert_eq!(count, 4);
    for f in [&a, &b] {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM runs")
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "prepared grants cannot bypass worker admission");
    }
    drop(fresh_app);
    fresh.store.pool.close().await;
    fresh.store.control_pool.close().await;
    server.abort();
    let _ = server.await;
    cleanup(a, &au, &aschema).await;
    cleanup(b, &bu, &bschema).await;
}
