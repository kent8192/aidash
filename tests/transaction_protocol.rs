use aidash::{
    Error, api,
    config::Config,
    federation::{Federation, Peer},
    registry::Registry,
    store::Store,
    transactions::{Manifest, coordinator, participant},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, Executor, postgres::PgConnection};
use std::sync::Arc;
use uuid::Uuid;

struct Node {
    f: Federation,
    server: Option<tokio::task::JoinHandle<()>>,
    database: String,
    admin: String,
    _capacity: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Node {
    async fn new(suffix: &str) -> Self {
        let admin = std::env::var("AIDASH_TEST_DATABASE_URL").unwrap();
        let database = format!("atomic_{}_{}", suffix, Uuid::new_v4().simple());
        PgConnection::connect(&admin)
            .await
            .unwrap()
            .execute(format!("CREATE DATABASE {database}").as_str())
            .await
            .unwrap();
        let mut url = reqwest::Url::parse(&admin).unwrap();
        url.set_path(&format!("/{database}"));
        let node_id = format!("aidash://atomic-{suffix}");
        let store = Store::connect(url.as_str(), node_id.clone()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let config = Config {
            node_id,
            endpoint: format!("http://{listen}"),
            listen,
            database_url: url.to_string(),
            nats_url: "nats://127.0.0.1:42270".into(),
            api_token: "atomic-operator-fixture-token".into(),
            web_dir: "web/dist".into(),
            lease_seconds: 30,
        };
        let f = Federation {
            registry: Registry::new(store.pool.clone()),
            store,
            config,
            client: reqwest::Client::new(),
            notify: Arc::new(tokio::sync::Notify::new()),
        };
        let app = api::router(f.clone());
        let server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        Self {
            f,
            server,
            database,
            admin,
            _capacity: None,
        }
    }
    async fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
    }
    async fn restart(&mut self) {
        self.stop().await;
        self.f.store.pool.close().await;
        self.f.store.control_pool.close().await;
        let store = Store::connect(&self.f.config.database_url, self.f.config.node_id.clone())
            .await
            .unwrap();
        self.f = Federation {
            registry: Registry::new(store.pool.clone()),
            store,
            notify: Arc::new(tokio::sync::Notify::new()),
            ..self.f.clone()
        };
        let listener = tokio::net::TcpListener::bind(self.f.config.listen)
            .await
            .unwrap();
        let app = api::router(self.f.clone());
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .f
            .client
            .request(method, format!("{}{path}", self.f.config.endpoint))
            .bearer_auth(&self.f.config.api_token)
            .timeout(std::time::Duration::from_secs(5));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let bytes = response.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn get(&self, path: &str) -> (u16, Value) {
        self.request(reqwest::Method::GET, path, None).await
    }
    async fn cleanup(mut self) {
        self.stop().await;
        self.f.store.pool.close().await;
        self.f.store.control_pool.close().await;
        PgConnection::connect(&self.admin)
            .await
            .unwrap()
            .execute(format!("DROP DATABASE {} WITH (FORCE)", self.database).as_str())
            .await
            .unwrap();
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}
async fn pair() -> (Node, Node, Manifest, Uuid, Uuid) {
    static CAPACITY: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
    let capacity = CAPACITY.clone().acquire_owned().await.unwrap();
    let mut a = Node::new("a").await;
    a._capacity = Some(capacity);
    let b = Node::new("b").await;
    for (local, remote) in [(&a, &b), (&b, &a)] {
        local
            .f
            .register_peer(Peer {
                node_id: remote.f.config.node_id.clone(),
                endpoint: remote.f.config.endpoint.clone(),
                credential_env: "AIDASH_SECRET_TEST_PEER".into(),
                protocol_version: "0.1".into(),
                enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(
            local
                .request(
                    reqwest::Method::POST,
                    "/api/transactions/trust",
                    Some(json!({"node_id":remote.f.config.node_id,"enabled":true}))
                )
                .await
                .0,
            200
        );
    }
    let wa =
        a.f.store
            .create_workspace("A", "Atomic state")
            .await
            .unwrap();
    let wb =
        b.f.store
            .create_workspace("B", "Atomic state")
            .await
            .unwrap();
    let manifest=serde_json::from_value(json!({"id":Uuid::new_v4(),"coordinator":a.f.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),"participants":[{"node_id":a.f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":wa.id,"expected_revision":0,"state":{"value":"new-a"}}]},{"node_id":b.f.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":wb.id,"expected_revision":0,"state":{"value":"new-b"}}]}]})).unwrap();
    (a, b, manifest, wa.id, wb.id)
}
async fn steps(node: &Node, id: Uuid, count: usize) {
    for _ in 0..count {
        coordinator::advance(&node.f, id).await.unwrap();
    }
}
async fn complete(node: &Node, id: Uuid) -> aidash::transactions::Status {
    for _ in 0..40 {
        let state = coordinator::advance(&node.f, id).await.unwrap();
        if state.complete {
            return state;
        }
    }
    panic!(
        "transaction did not finish: {:?}",
        coordinator::status(&node.f, id).await.unwrap()
    );
}
async fn unavailable(node: &Node, workspace: Uuid) {
    for path in [
        "/api/state".to_string(),
        "/api/registry".into(),
        format!("/api/workspaces/{workspace}"),
        "/api/events".into(),
        "/.well-known/aidash".into(),
    ] {
        assert_eq!(node.get(&path).await.0, 503, "{path}");
    }
    assert_eq!(node.get("/health").await.0, 200);
    let (session_status, session) = node.get("/api/session").await;
    assert_eq!(session_status, 200);
    assert_eq!(session["access"]["kind"], "operator");
    assert_eq!(node.get("/api/transactions/participants").await.0, 200);
    assert!(matches!(
        aidash::harness::Harness {
            federation: node.f.clone()
        }
        .worker_once()
        .await,
        Err(Error::TransactionPending)
    ));
    assert!(matches!(
        aidash::generation::provision::reconcile(&node.f).await,
        Err(Error::TransactionPending)
    ));
    assert!(
        node.f
            .store
            .update_state(workspace, 0, json!({"bypass":true}))
            .await
            .is_err(),
        "statement trigger must reject a direct write"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn two_node_commit_hides_partial_application_and_releases_only_after_all_apply() {
    let (a, b, manifest, wa, wb) = pair().await;
    coordinator::submit(&a.f, &manifest).await.unwrap();
    steps(&a, manifest.id, 2).await;
    unavailable(&a, wa).await;
    unavailable(&b, wb).await;
    assert!(matches!(
        a.f.request::<Value>(
            &b.f.config.node_id,
            reqwest::Method::POST,
            "/discover",
            Some(&json!({}))
        )
        .await,
        Err(Error::TransactionPending)
    ));
    steps(&a, manifest.id, 2).await; // Both prepared; speculative writes rolled back.
    assert_eq!(a.f.store.workspace(wa).await.unwrap().state, json!({}));
    assert_eq!(b.f.store.workspace(wb).await.unwrap().state, json!({}));
    steps(&a, manifest.id, 1).await; // Durable commit precedes any application.
    assert_eq!(
        coordinator::status(&a.f, manifest.id)
            .await
            .unwrap()
            .decision
            .as_deref(),
        Some("COMMIT")
    );
    assert!(coordinator::abort(&a.f, manifest.id).await.is_err());
    assert!(
        sqlx::query("UPDATE atomic_coordinators SET decision='ABORT' WHERE id=$1")
            .bind(manifest.id)
            .execute(&a.f.store.control_pool)
            .await
            .is_err()
    );
    steps(&a, manifest.id, 1).await;
    assert_eq!(
        a.f.store.workspace(wa).await.unwrap().state["value"],
        "new-a"
    );
    assert_eq!(b.f.store.workspace(wb).await.unwrap().state, json!({}));
    unavailable(&a, wa).await;
    unavailable(&b, wb).await;
    steps(&a, manifest.id, 1).await;
    assert_eq!(
        b.f.store.workspace(wb).await.unwrap().state["value"],
        "new-b"
    );
    assert!(
        !coordinator::status(&a.f, manifest.id)
            .await
            .unwrap()
            .visible
    );
    steps(&a, manifest.id, 1).await; // Visibility certificate, barriers still retained.
    assert!(
        coordinator::status(&a.f, manifest.id)
            .await
            .unwrap()
            .visible
    );
    steps(&a, manifest.id, 1).await;
    assert_eq!(a.get(&format!("/api/workspaces/{wa}")).await.0, 200);
    assert_eq!(b.get(&format!("/api/workspaces/{wb}")).await.0, 503);
    let done = complete(&a, manifest.id).await;
    assert!(done.visible);
    for (node, workspace) in [(&a, wa), (&b, wb)] {
        assert_eq!(
            node.get(&format!("/api/workspaces/{workspace}")).await.0,
            200
        );
        assert_eq!(
            participant::finish(&node.f, &a.f.config.node_id, &manifest)
                .await
                .unwrap()
                .phase,
            "COMMITTED"
        );
        let updates: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE workspace_id=$1 AND kind='workspace.updated'",
        )
        .bind(workspace)
        .fetch_one(&node.f.store.pool)
        .await
        .unwrap();
        assert_eq!(updates, 1);
    }
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn stale_prepare_aborts_every_node_and_delayed_reserve_cannot_resurrect_it() {
    let (a, b, mut manifest, wa, wb) = pair().await;
    if let aidash::transactions::Mutation::WorkspaceState {
        expected_revision, ..
    } = &mut manifest.participants[1].mutations[0]
    {
        *expected_revision = 9;
    }
    coordinator::submit(&a.f, &manifest).await.unwrap();
    let done = complete(&a, manifest.id).await;
    assert_eq!(done.decision.as_deref(), Some("ABORT"));
    assert!(!done.visible);
    for (node, workspace) in [(&a, wa), (&b, wb)] {
        assert_eq!(
            node.f.store.workspace(workspace).await.unwrap().state,
            json!({})
        );
        assert_eq!(
            node.get(&format!("/api/workspaces/{workspace}")).await.0,
            200
        );
        assert_eq!(
            participant::reserve(&node.f, &a.f.config.node_id, &manifest)
                .await
                .unwrap()
                .phase,
            "ABORTED"
        );
        assert_eq!(
            participant::finish(&node.f, &a.f.config.node_id, &manifest)
                .await
                .unwrap()
                .phase,
            "ABORTED"
        );
    }
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn partition_after_commit_retains_barriers_and_restart_recovers_the_same_decision() {
    let (mut a, mut b, manifest, wa, wb) = pair().await;
    coordinator::submit(&a.f, &manifest).await.unwrap();
    steps(&a, manifest.id, 5).await;
    b.stop().await;
    steps(&a, manifest.id, 2).await;
    let waiting = coordinator::status(&a.f, manifest.id).await.unwrap();
    assert_eq!(waiting.decision.as_deref(), Some("COMMIT"));
    assert!(!waiting.visible);
    assert!(waiting.last_error.is_some());
    unavailable(&a, wa).await;
    a.restart().await;
    b.restart().await;
    unavailable(&a, wa).await;
    unavailable(&b, wb).await;
    let done = complete(&a, manifest.id).await;
    assert_eq!(done.decision.as_deref(), Some("COMMIT"));
    assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn overlapping_coordinators_use_the_same_node_order_without_lost_updates() {
    let (a, b, first, wa, wb) = pair().await;
    let mut second = first.clone();
    second.id = Uuid::new_v4();
    second.coordinator = b.f.config.node_id.clone();
    coordinator::submit(&a.f, &first).await.unwrap();
    coordinator::submit(&b.f, &second).await.unwrap();
    steps(&a, first.id, 1).await;
    steps(&b, second.id, 1).await;
    let waiting = coordinator::status(&b.f, second.id).await.unwrap();
    assert!(waiting.decision.is_none());
    assert!(waiting.last_error.is_some());
    assert_eq!(
        complete(&a, first.id).await.decision.as_deref(),
        Some("COMMIT")
    );
    assert_eq!(
        complete(&b, second.id).await.decision.as_deref(),
        Some("ABORT")
    );
    assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn registry_workspace_task_execution_and_artifact_commit_together_once() {
    use aidash::{
        domain::{NewTask, qualified_agent},
        registry::Entry,
    };
    let (a, b, mut manifest, wa, _wb) = pair().await;
    let agent:Entry=serde_json::from_value(json!({"id":"executor","version":"1.0.0","kind":"agent","name":{"en":"Executor"},"description":{"en":"Atomic fixture"},"config":{"model":{"id":"fixture","version":"1.0.0"},"instructions":"Atomic execution","tools":[],"skills":[]}})).unwrap();
    let owner = qualified_agent(&b.f.config.node_id, &agent.id, &agent.version);
    let task =
        a.f.store
            .create_task(
                wa,
                &NewTask {
                    title: "Atomic task".into(),
                    description: "Finish with the remote execution".into(),
                    requirements: json!({}),
                    dependencies: vec![],
                    parent_id: None,
                },
                "operator",
                None,
            )
            .await
            .unwrap();
    a.f.store.claim(task.id, 0, &owner, &agent).await.unwrap();
    let task =
        a.f.store
            .transition(task.id, 1, &owner, "RUNNING")
            .await
            .unwrap();
    sqlx::query("INSERT INTO delegations(task_id,node_id,agent_id,agent_version,delivered) VALUES($1,$2,$3,$4,true)").bind(task.id).bind(&b.f.config.node_id).bind(&agent.id).bind(&agent.version).execute(&a.f.store.pool).await.unwrap();
    let run =
        b.f.store
            .accept_run(&task, &a.f.config.node_id, &agent.id, &agent.version)
            .await
            .unwrap();
    sqlx::query("UPDATE runs SET phase='TOOL_CALL',pending=$2 WHERE id=$1")
        .bind(run.id)
        .bind(json!({"response":{"text":"one committed result","tool_calls":[],"input_tokens":0,"output_tokens":0},"cursor":0}))
        .execute(&b.f.store.pool)
        .await
        .unwrap();
    manifest.participants[0].mutations.push(serde_json::from_value(json!({"kind":"registry_register","entry":{"id":"atomic-skill","version":"1.0.0","kind":"skill","name":{"en":"Atomic skill"},"description":{"en":"Prepared registration"},"config":{"instructions":"Complete atomic work"}}})).unwrap());
    manifest.participants[0].mutations.push(serde_json::from_value(json!({"kind":"complete_task","task_id":task.id,"expected_revision":task.revision,"artifact":{"kind":"text","name":"Atomic result","content":"one committed result"}})).unwrap());
    manifest.participants[1].mutations.push(serde_json::from_value(json!({"kind":"finish_run","run_id":run.id,"task_id":task.id,"expected_revision":run.revision})).unwrap());
    let mut invalid = manifest.clone();
    invalid.id = Uuid::new_v4();
    sqlx::query(
        "UPDATE runs SET pending=jsonb_set(pending,'{response,tool_calls}',$2) WHERE id=$1",
    )
    .bind(run.id)
    .bind(json!([{"id":"unfinished","name":"http","arguments":{}}]))
    .execute(&b.f.store.pool)
    .await
    .unwrap();
    coordinator::submit(&a.f, &invalid).await.unwrap();
    assert_eq!(
        complete(&a, invalid.id).await.decision.as_deref(),
        Some("ABORT")
    );
    assert_eq!(a.f.store.task(task.id).await.unwrap().status, "RUNNING");
    assert!(a.f.store.snapshot(wa).await.unwrap().artifacts.is_empty());
    sqlx::query("UPDATE runs SET pending=jsonb_set(pending,'{response,tool_calls}','[]'::jsonb) WHERE id=$1").bind(run.id).execute(&b.f.store.pool).await.unwrap();
    coordinator::submit(&a.f, &manifest).await.unwrap();
    assert_eq!(
        complete(&a, manifest.id).await.decision.as_deref(),
        Some("COMMIT")
    );
    for _ in 0..2 {
        participant::finish(&a.f, &a.f.config.node_id, &manifest)
            .await
            .unwrap();
        participant::finish(&b.f, &a.f.config.node_id, &manifest)
            .await
            .unwrap();
    }
    assert_eq!(a.f.store.task(task.id).await.unwrap().status, "COMPLETED");
    assert_eq!(b.f.store.run(run.id).await.unwrap().phase, "COMPLETED");
    assert_eq!(
        a.f.registry
            .get("atomic-skill", "1.0.0")
            .await
            .unwrap()
            .kind,
        "skill"
    );
    let artifacts = a.f.store.snapshot(wa).await.unwrap().artifacts;
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].created_by, owner);
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn participant_pulls_only_durable_decisions_and_never_guesses_after_timeout() {
    let (mut a, b, manifest, wa, wb) = pair().await;
    coordinator::submit(&a.f, &manifest).await.unwrap();
    steps(&a, manifest.id, 4).await;
    a.stop().await;
    assert_eq!(participant::recover_once(&b.f).await.unwrap(), 0);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 0);
    unavailable(&b, wb).await;
    a.restart().await;
    steps(&a, manifest.id, 1).await;
    // The coordinator has not sent apply, but a participant can recover its vote.
    assert_eq!(participant::recover_once(&b.f).await.unwrap(), 1);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
    unavailable(&a, wa).await;
    unavailable(&b, wb).await;
    assert_eq!(
        complete(&a, manifest.id).await.decision.as_deref(),
        Some("COMMIT")
    );
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn an_existing_sse_stream_waits_for_atomic_visibility_before_emitting_changes() {
    use axum::{body::Body, http::Request};
    use futures_util::StreamExt;
    use tower::ServiceExt;
    let (a, b, manifest, wa, _wb) = pair().await;
    let response = api::router(a.f.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/events/stream?workspace_id={wa}"))
                .header("authorization", format!("Bearer {}", a.f.config.api_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("workspace.created"));
    coordinator::submit(&a.f, &manifest).await.unwrap();
    steps(&a, manifest.id, 6).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), stream.next())
            .await
            .is_err()
    );
    complete(&a, manifest.id).await;
    let event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&event).contains("new-a"));
    drop(stream);
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn peer_trust_denial_aborts_promptly_and_revocation_preserves_admitted_recovery() {
    let (a, b, manifest, wa, wb) = pair().await;
    let set_trust = |enabled| json!({"node_id":a.f.config.node_id,"enabled":enabled});
    assert_eq!(
        b.request(
            reqwest::Method::POST,
            "/api/transactions/trust",
            Some(set_trust(false))
        )
        .await
        .0,
        200
    );
    coordinator::submit(&a.f, &manifest).await.unwrap();
    steps(&a, manifest.id, 2).await;
    // A terminal peer rejection must abort immediately, without waiting for the deadline.
    assert_eq!(
        coordinator::status(&a.f, manifest.id)
            .await
            .unwrap()
            .decision
            .as_deref(),
        Some("ABORT")
    );
    complete(&a, manifest.id).await;
    assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 0);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 0);

    assert_eq!(
        b.request(
            reqwest::Method::POST,
            "/api/transactions/trust",
            Some(set_trust(true))
        )
        .await
        .0,
        200
    );
    let mut admitted = manifest.clone();
    admitted.id = Uuid::new_v4();
    // An authenticated peer cannot claim that a different node coordinates its manifest.
    let mut forged = admitted.clone();
    forged.coordinator = b.f.config.node_id.clone();
    let response =
        a.f.client
            .post(format!(
                "{}/federation/v0.1/transactions/reserve",
                b.f.config.endpoint
            ))
            .bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
            .header("x-aidash-node", &a.f.config.node_id)
            .header("x-aidash-protocol", "0.1")
            .json(&forged)
            .send()
            .await
            .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(b.get(&format!("/api/workspaces/{wb}")).await.0, 200);
    coordinator::submit(&a.f, &admitted).await.unwrap();
    steps(&a, admitted.id, 2).await;
    assert_eq!(
        b.request(
            reqwest::Method::POST,
            "/api/transactions/trust",
            Some(set_trust(false))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        complete(&a, admitted.id).await.decision.as_deref(),
        Some("COMMIT")
    );
    assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn every_durable_transition_survives_fresh_pools_and_http_servers() {
    let (mut a, mut b, manifest, wa, wb) = pair().await;
    coordinator::submit(&a.f, &manifest).await.unwrap();
    let mut transitions = 0;
    loop {
        a.restart().await;
        b.restart().await;
        let state = coordinator::advance(&a.f, manifest.id).await.unwrap();
        transitions += 1;
        if !state.visible {
            // No node may expose a new value before the global visibility decision.
            for (node, workspace) in [(&a, wa), (&b, wb)] {
                let (status, value) = node.get(&format!("/api/workspaces/{workspace}")).await;
                assert!(
                    status == 503 || (status == 200 && value["workspace"]["revision"] == 0),
                    "{status}: {value}"
                );
            }
        }
        if state.complete {
            break;
        }
        assert!(transitions < 20);
    }
    assert_eq!(transitions, 11);
    assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn undecided_deadline_aborts_without_publishing_prepared_mutations() {
    let (a, b, mut manifest, wa, wb) = pair().await;
    manifest.deadline = Utc::now() + Duration::seconds(2);
    coordinator::submit(&a.f, &manifest).await.unwrap();
    steps(&a, manifest.id, 4).await;
    let remaining = (manifest.deadline - Utc::now())
        .to_std()
        .unwrap_or_default();
    tokio::time::sleep(remaining + std::time::Duration::from_millis(10)).await;
    assert_eq!(
        complete(&a, manifest.id).await.decision.as_deref(),
        Some("ABORT")
    );
    assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 0);
    assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 0);
    a.cleanup().await;
    b.cleanup().await;
}

struct WorkerProcess(std::process::Child);
impl WorkerProcess {
    fn start(node: &Node) -> Self {
        Self(
            std::process::Command::new(env!("CARGO_BIN_EXE_aidash"))
                .arg("worker")
                .env("DATABASE_URL", &node.f.config.database_url)
                .env("AIDASH_NODE_ID", &node.f.config.node_id)
                .env("AIDASH_ENDPOINT", &node.f.config.endpoint)
                .env("AIDASH_API_TOKEN", &node.f.config.api_token)
                .env("RUST_LOG", "aidash=error")
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for WorkerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn actual_worker_sigkill_after_commit_recovers_without_replaying_effects() {
    let (a, mut b, manifest, wa, wb) = pair().await;
    coordinator::submit(&a.f, &manifest).await.unwrap();
    steps(&a, manifest.id, 5).await;
    b.stop().await;
    let worker = WorkerProcess::start(&a);
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let phase: String =
                sqlx::query_scalar("SELECT phase FROM atomic_participants WHERE id=$1")
                    .bind(manifest.id)
                    .fetch_one(&a.f.store.control_pool)
                    .await
                    .unwrap();
            if phase == "APPLIED" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    drop(worker); // SIGKILL the real coordinator worker, including its recovery loops.
    assert_eq!(
        coordinator::status(&a.f, manifest.id)
            .await
            .unwrap()
            .decision
            .as_deref(),
        Some("COMMIT")
    );
    unavailable(&a, wa).await;
    b.restart().await;
    let restarted_a = WorkerProcess::start(&a);
    let restarted_b = WorkerProcess::start(&b);
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if coordinator::status(&a.f, manifest.id)
                .await
                .unwrap()
                .complete
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    drop(restarted_a);
    drop(restarted_b);
    for (node, workspace) in [(&a, wa), (&b, wb)] {
        assert_eq!(node.f.store.workspace(workspace).await.unwrap().revision, 1);
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE workspace_id=$1 AND kind='workspace.updated'",
        )
        .bind(workspace)
        .fetch_one(&node.f.store.pool)
        .await
        .unwrap();
        assert_eq!(events, 1);
    }
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn unreachable_aborted_transactions_cannot_starve_later_local_work() {
    let (a, mut b, manifest, wa, _wb) = pair().await;
    for _ in 0..33 {
        let mut old = manifest.clone();
        old.id = Uuid::new_v4();
        coordinator::submit(&a.f, &old).await.unwrap();
        coordinator::abort(&a.f, old.id).await.unwrap();
        steps(&a, old.id, 1).await;
    }
    b.stop().await;
    let mut local = manifest.clone();
    local.id = Uuid::new_v4();
    local.participants.truncate(1);
    coordinator::submit(&a.f, &local).await.unwrap();
    for _ in 0..20 {
        coordinator::recover_once(&a.f).await.unwrap();
        if coordinator::status(&a.f, local.id).await.unwrap().complete {
            break;
        }
    }
    assert!(coordinator::status(&a.f, local.id).await.unwrap().complete);
    assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
    a.cleanup().await;
    b.cleanup().await;
}
