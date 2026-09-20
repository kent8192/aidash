mod common;
use aidash::api;
use chrono::{Duration, Utc};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn atomic_submission_validates_the_entire_manifest_before_creating_work() {
    let (f, url, schema) = setup().await;
    let app = api::router(f.clone());
    let (_, subject, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
    let (session_status, session) =
        request(&app, &subject, "GET", "/api/session", Value::Null).await;
    assert_eq!(session_status, 200);
    assert_eq!(
        session["access"],
        json!({"kind":"subject","tenant":"acme","subject":"alice"})
    );
    assert_eq!(
        request(&app, &subject, "GET", "/api/transactions", Value::Null)
            .await
            .0,
        403
    );
    let workspace = f
        .store
        .create_workspace("Atomic", "Commit together")
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let manifest = json!({"id":id,"coordinator":f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),"participants":[{"node_id":f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":workspace.id,"expected_revision":0,"state":{"result":"committed"}}]}]});
    let (status, response) = request(
        &app,
        &f.config.api_token,
        "POST",
        "/api/transactions",
        manifest.clone(),
    )
    .await;
    assert_eq!(status, 202, "{response}");
    assert_eq!(response["id"], json!(id));
    assert_eq!(response["decision"], Value::Null);
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/transactions",
            manifest.clone()
        )
        .await,
        (202, response)
    );
    let mut changed = manifest.clone();
    changed["participants"][0]["mutations"][0]["state"] = json!({"different":true});
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/transactions",
            changed
        )
        .await
        .0,
        409
    );
    let mut external = manifest;
    external["id"] = json!(Uuid::new_v4());
    external["participants"][0]["mutations"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"external_tool","endpoint":"http://127.0.0.1:9/effect"}));
    assert_eq!(
        request(
            &app,
            &f.config.api_token,
            "POST",
            "/api/transactions",
            external
        )
        .await
        .0,
        422
    );
    assert_eq!(
        f.store.workspace(workspace.id).await.unwrap().state,
        json!({})
    );
    cleanup(f, &url, &schema).await;
}
