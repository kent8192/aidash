use crate::{
    Error, Result,
    config::{PROTOCOL_VERSION, secret},
    domain::*,
    federation::{Federation, Offer, Peer},
    registry::{EntityRef, Entry, Package, PackageRecord, Search},
    store::Invocation,
    tool::required,
};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::HeaderMap,
    middleware::{self, Next},
    response::{
        Response, Sse,
        sse::{Event as SseEvent, KeepAlive},
    },
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
use uuid::Uuid;

pub fn router(f: Federation) -> Router {
    let api = Router::new()
        .route("/state", get(state))
        .route("/registry", get(registry_list).post(registry_create))
        .route("/registry/{id}/{version}", get(registry_get))
        .route("/workspaces", post(workspace_create))
        .route(
            "/workspaces/{id}",
            get(workspace_get).patch(workspace_update),
        )
        .route("/workspaces/{id}/tasks", post(task_create))
        .route("/workspaces/{id}/messages", post(message_create))
        .route("/tasks/{id}/claim", post(task_claim))
        .route("/tasks/{id}/delegate", post(task_delegate))
        .route("/conversations", post(conversation_create))
        .route("/runs/{id}", get(run_get))
        .route("/runs/{id}/control", post(run_control))
        .route("/runs/{id}/message", post(run_message))
        .route("/human-requests/{id}/answer", post(human_answer))
        .route("/peers", post(peer_create))
        .route("/discover", post(discover))
        .route("/mesh", get(mesh))
        .route("/remote", post(remote_action))
        .route("/marketplace", get(marketplace).post(package_publish))
        .route("/marketplace/{id}/{version}/install", post(package_install))
        .route("/events", get(events))
        .route("/events/stream", get(stream))
        .route_layer(middleware::from_fn_with_state(f.clone(), api_auth));
    let federation = Router::new()
        .route("/discover", post(peer_discover))
        .route("/offers", post(peer_offer))
        .route("/workspace", post(peer_workspace))
        .route("/observe", get(peer_observe))
        .route("/control", post(peer_control))
        .route_layer(middleware::from_fn_with_state(f.clone(), peer_auth));
    let web = tower_http::services::ServeDir::new(&f.config.web_dir).not_found_service(
        tower_http::services::ServeFile::new(format!("{}/index.html", f.config.web_dir)),
    );
    Router::new()
        .route("/health", get(health))
        .route("/.well-known/aidash", get(identity))
        .nest("/api", api)
        .nest("/federation/v0.1", federation)
        .fallback_service(web)
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .with_state(f)
}
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
fn same_secret(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(a.as_bytes());
    let b = Sha256::digest(b.as_bytes());
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
async fn api_auth(State(f): State<Federation>, request: Request, next: Next) -> Result<Response> {
    if bearer(request.headers()).is_none_or(|s| !same_secret(s, &f.config.api_token)) {
        return Err(Error::Unauthorized);
    }
    Ok(next.run(request).await)
}
async fn peer_auth(State(f): State<Federation>, request: Request, next: Next) -> Result<Response> {
    let headers = request.headers();
    if headers
        .get("x-aidash-protocol")
        .and_then(|h| h.to_str().ok())
        != Some(PROTOCOL_VERSION)
    {
        return Err(Error::Invalid("unsupported federation protocol".into()));
    }
    let node = peer_node(headers)?;
    let peer = f.peer(node).await?;
    if bearer(headers)
        .is_none_or(|s| !secret(&peer.credential_env).is_ok_and(|key| same_secret(s, &key)))
    {
        return Err(Error::Unauthorized);
    }
    Ok(next.run(request).await)
}
fn peer_node(headers: &HeaderMap) -> Result<&str> {
    headers
        .get("x-aidash-node")
        .and_then(|h| h.to_str().ok())
        .ok_or(Error::Unauthorized)
}
async fn health(State(f): State<Federation>) -> Result<Json<Value>> {
    sqlx::query("SELECT 1").execute(&f.store.pool).await?;
    Ok(Json(json!({"status":"ok","node_id":f.config.node_id})))
}
async fn identity(State(f): State<Federation>) -> Result<Json<Value>> {
    let clusters = f
        .registry
        .list(&Search {
            kind: Some("cluster".into()),
            ..Default::default()
        })
        .await?
        .into_iter()
        .map(|e| format!("{}@{}", e.id, e.version))
        .collect();
    Ok(Json(json!(f.config.identity(clusters))))
}
async fn state(State(f): State<Federation>) -> Result<Json<Value>> {
    let records = f.registry.list(&Search::default()).await?;
    let events:Vec<crate::domain::Event>=sqlx::query_as("SELECT sequence,id,node_id,workspace_id,kind,data,created_at FROM (SELECT * FROM events ORDER BY sequence DESC LIMIT 100) e ORDER BY sequence").fetch_all(&f.store.pool).await?;
    let human: Vec<HumanRequest> =
        sqlx::query_as("SELECT * FROM human_requests ORDER BY created_at DESC LIMIT 500")
            .fetch_all(&f.store.pool)
            .await?;
    let conversations: Vec<Conversation> =
        sqlx::query_as("SELECT * FROM conversations ORDER BY created_at DESC LIMIT 500")
            .fetch_all(&f.store.pool)
            .await?;
    let artifacts: Vec<Artifact> =
        sqlx::query_as("SELECT * FROM artifacts ORDER BY created_at DESC LIMIT 500")
            .fetch_all(&f.store.pool)
            .await?;
    let installations: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM installations i ORDER BY installed_at DESC")
            .fetch_all(&f.store.pool)
            .await?;
    Ok(Json(
        json!({"node":f.config.identity(vec![]),"registry":records,"workspaces":f.store.workspaces().await?,"tasks":f.store.tasks(None).await?,"runs":f.store.runs().await?,"human_requests":human,"conversations":conversations,"peers":f.peers().await?,"events":events,"artifacts":artifacts,"installations":installations}),
    ))
}
async fn registry_list(
    State(f): State<Federation>,
    Query(search): Query<Search>,
) -> Result<Json<Vec<Entry>>> {
    Ok(Json(f.registry.list(&search).await?))
}
async fn registry_get(
    State(f): State<Federation>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Entry>> {
    Ok(Json(f.registry.get(&id, &version).await?))
}
async fn registry_create(
    State(f): State<Federation>,
    Json(entry): Json<Entry>,
) -> Result<Json<Entry>> {
    let entry = f.registry.register(entry).await?;
    f.store
        .emit(
            None,
            "registry.registered",
            json!({"id":entry.id,"version":entry.version,"kind":entry.kind}),
        )
        .await?;
    Ok(Json(entry))
}
#[derive(Deserialize)]
struct WorkspaceInput {
    title: String,
    goal: String,
}
async fn workspace_create(
    State(f): State<Federation>,
    Json(input): Json<WorkspaceInput>,
) -> Result<Json<Workspace>> {
    Ok(Json(
        f.store.create_workspace(&input.title, &input.goal).await?,
    ))
}
async fn workspace_get(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceSnapshot>> {
    Ok(Json(f.store.snapshot(id).await?))
}
#[derive(Deserialize)]
struct StateInput {
    revision: i64,
    state: Value,
}
async fn workspace_update(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    Json(input): Json<StateInput>,
) -> Result<Json<Workspace>> {
    Ok(Json(
        f.store
            .update_state(id, input.revision, input.state)
            .await?,
    ))
}
async fn task_create(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<NewTask>,
) -> Result<Json<Task>> {
    Ok(Json(
        f.store
            .create_task(
                id,
                &input,
                "human",
                headers.get("idempotency-key").and_then(|h| h.to_str().ok()),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
struct MessageInput {
    content: String,
}
async fn message_create(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    Json(input): Json<MessageInput>,
) -> Result<Json<Value>> {
    f.store.message(id, "human", &input.content, None).await?;
    Ok(Json(json!({"sent":true})))
}
#[derive(Deserialize)]
struct ClaimInput {
    revision: i64,
    agent: EntityRef,
}
async fn task_claim(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    Json(input): Json<ClaimInput>,
) -> Result<Json<Task>> {
    let entry = f
        .registry
        .get(&input.agent.id, &input.agent.version)
        .await?;
    let owner = qualified_agent(&f.config.node_id, &entry.id, &entry.version);
    let task = f.store.claim(id, input.revision, &owner, &entry).await?;
    f.store
        .accept_run(&task, &f.config.node_id, &entry.id, &entry.version)
        .await?;
    Ok(Json(task))
}
#[derive(Deserialize)]
struct DelegateInput {
    node_id: String,
    agent: EntityRef,
}
async fn task_delegate(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    Json(input): Json<DelegateInput>,
) -> Result<Json<Value>> {
    Ok(Json(json!(
        f.delegate(id, &input.node_id, &input.agent).await?
    )))
}
#[derive(Deserialize)]
struct ConversationInput {
    title: String,
    goal: String,
    target: EntityRef,
    target_kind: String,
}
async fn conversation_create(
    State(f): State<Federation>,
    Json(input): Json<ConversationInput>,
) -> Result<Json<Value>> {
    let target = f
        .registry
        .get(&input.target.id, &input.target.version)
        .await?;
    if target.kind != input.target_kind || !matches!(target.kind.as_str(), "agent" | "cluster") {
        return Err(Error::Invalid(
            "conversation target must be an agent or cluster".into(),
        ));
    }
    let agent = if target.kind == "agent" {
        input.target.clone()
    } else {
        serde_json::from_value::<EntityRef>(target.config["coordinator"].clone()).map_err(|_| {
            Error::Invalid("cluster requires an explicit coordinator agent reference".into())
        })?
    };
    let entry = f.registry.get(&agent.id, &agent.version).await?;
    if entry.kind != "agent" {
        return Err(Error::Invalid("coordinator must be an agent".into()));
    }
    let workspace = f.store.create_workspace(&input.title, &input.goal).await?;
    let mut tx = f.store.pool.begin().await?;
    let c:Conversation=sqlx::query_as("INSERT INTO conversations(id,workspace_id,target,target_kind) VALUES($1,$2,$3,$4) RETURNING *")
        .bind(Uuid::new_v4()).bind(workspace.id).bind(format!("{}@{}",input.target.id,input.target.version)).bind(input.target_kind).fetch_one(&mut *tx).await?;
    f.store
        .event(
            &mut tx,
            Some(workspace.id),
            "conversation.created",
            json!(c),
        )
        .await?;
    tx.commit().await?;
    f.store
        .message(workspace.id, "human", &input.goal, None)
        .await?;
    let task = f
        .store
        .create_task(
            workspace.id,
            &NewTask {
                title: input.title,
                description: input.goal,
                requirements: json!({}),
                dependencies: vec![],
                parent_id: None,
            },
            "human",
            None,
        )
        .await?;
    let delegation = f.delegate(task.id, &f.config.node_id, &agent).await?;
    Ok(Json(
        json!({"conversation":c,"workspace":workspace,"task":task,"delegation":delegation}),
    ))
}
async fn run_get(State(f): State<Federation>, Path(id): Path<Uuid>) -> Result<Json<Value>> {
    let run = f.store.run(id).await?;
    let invocations: Vec<Invocation> =
        sqlx::query_as("SELECT * FROM invocations WHERE run_id=$1 ORDER BY created_at")
            .bind(id)
            .fetch_all(&f.store.pool)
            .await?;
    Ok(Json(
        json!({"run":run,"invocations":invocations,"memory":f.store.memory(&run).await?}),
    ))
}
#[derive(Deserialize)]
struct ControlInput {
    action: String,
}
async fn run_control(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    Json(input): Json<ControlInput>,
) -> Result<Json<Run>> {
    let r = f.store.control(id, &input.action).await?;
    f.notify.notify_waiters();
    Ok(Json(r))
}
async fn run_message(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    Json(input): Json<MessageInput>,
) -> Result<Json<Value>> {
    let run = f.store.run(id).await?;
    let home = crate::federation::Home { federation: f, run };
    home.human_message(&format!("human:{}", Uuid::new_v4()), &input.content)
        .await?;
    Ok(Json(json!({"sent":true})))
}
async fn human_answer(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
    Json(response): Json<Value>,
) -> Result<Json<HumanRequest>> {
    let h = f.store.answer(id, response).await?;
    f.notify.notify_waiters();
    Ok(Json(h))
}
async fn peer_create(State(f): State<Federation>, Json(peer): Json<Peer>) -> Result<Json<Peer>> {
    Ok(Json(f.register_peer(peer).await?))
}
async fn discover(State(f): State<Federation>, Json(query): Json<Search>) -> Result<Json<Value>> {
    Ok(Json(json!(f.discover(&query).await?)))
}
async fn marketplace(
    State(f): State<Federation>,
    Query(query): Query<Search>,
) -> Result<Json<Vec<PackageRecord>>> {
    let all: Vec<PackageRecord> = sqlx::query_as("SELECT * FROM packages ORDER BY id,version")
        .fetch_all(&f.store.pool)
        .await?;
    Ok(Json(
        all.into_iter()
            .filter(|p| {
                serde_json::from_value::<Package>(p.manifest.clone())
                    .is_ok_and(|p| query.matches(&p.entity))
            })
            .collect(),
    ))
}
async fn package_publish(
    State(f): State<Federation>,
    Json(package): Json<Package>,
) -> Result<Json<PackageRecord>> {
    let p = f.registry.publish(&f.store.pool, package).await?;
    f.store
        .emit(
            None,
            "package.published",
            json!({"id":p.id,"version":p.version}),
        )
        .await?;
    Ok(Json(p))
}
#[derive(Deserialize)]
struct InstallInput {
    digest: String,
    #[serde(default = "empty_object")]
    config: Value,
}
async fn package_install(
    State(f): State<Federation>,
    Path((id, version)): Path<(String, String)>,
    Json(input): Json<InstallInput>,
) -> Result<Json<Entry>> {
    let e = f
        .registry
        .install(&f.store.pool, &id, &version, &input.digest, input.config)
        .await?;
    f.store
        .emit(
            None,
            "package.installed",
            json!({"id":id,"version":version}),
        )
        .await?;
    Ok(Json(e))
}
#[derive(Default, Deserialize)]
struct EventQuery {
    #[serde(default)]
    after: i64,
    workspace_id: Option<Uuid>,
}
async fn events(
    State(f): State<Federation>,
    Query(q): Query<EventQuery>,
) -> Result<Json<Vec<crate::domain::Event>>> {
    Ok(Json(f.store.events(q.after, q.workspace_id, 500).await?))
}
async fn stream(
    State(f): State<Federation>,
    headers: HeaderMap,
    Query(q): Query<EventQuery>,
) -> Sse<impl futures_util::Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let mut cursor = headers
        .get("last-event-id")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(q.after);
    let stream = async_stream::stream! {
        loop {
            match f.store.events(cursor,q.workspace_id,100).await {
                Ok(events)=>for event in events{cursor=event.sequence;yield Ok(SseEvent::default().id(cursor.to_string()).event("mesh").data(event.cloud_event().to_string()));},
                Err(e)=>{tracing::error!(error=%e,"SSE read failed");yield Ok(SseEvent::default().event("error").data("event stream interrupted"));break;}
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn peer_discover(
    State(f): State<Federation>,
    Json(mut query): Json<Search>,
) -> Result<Json<Vec<Entry>>> {
    query.kind = Some("agent".into());
    Ok(Json(f.registry.list(&query).await?))
}
async fn peer_offer(
    State(f): State<Federation>,
    headers: HeaderMap,
    Json(offer): Json<Offer>,
) -> Result<Json<Run>> {
    let node = peer_node(&headers)?;
    let entry = f
        .registry
        .get(&offer.agent.id, &offer.agent.version)
        .await?;
    let query: Search = serde_json::from_value(offer.task.requirements.clone())?;
    if entry.kind != "agent" || !query.matches(&entry) {
        return Err(Error::Invalid(
            "offered task requirements do not match the agent".into(),
        ));
    }
    let run = f
        .store
        .accept_run(&offer.task, node, &offer.agent.id, &offer.agent.version)
        .await?;
    f.notify.notify_waiters();
    Ok(Json(run))
}
#[derive(Deserialize)]
struct WorkspaceCommand {
    task_id: Uuid,
    agent: EntityRef,
    operation: String,
    data: Value,
}
async fn peer_workspace(
    State(f): State<Federation>,
    headers: HeaderMap,
    Json(command): Json<WorkspaceCommand>,
) -> Result<Json<Value>> {
    let node = peer_node(&headers)?;
    f.authorize_task(node, command.task_id, &command.agent)
        .await?;
    let owner = qualified_agent(node, &command.agent.id, &command.agent.version);
    let task = f.store.task(command.task_id).await?;
    let d = &command.data;
    let key = || -> Result<String> { Ok(format!("{node}:{}:{}", task.id, required(d, "key")?)) };
    if !matches!(command.operation.as_str(), "snapshot" | "task" | "claim")
        && !(task.owner.is_none()
            && (command.operation == "human_message"
                || (command.operation == "transition" && d["status"] == "CANCELLED")))
        && task.owner.as_deref() != Some(&owner)
    {
        return Err(Error::Unauthorized);
    }
    let result = match command.operation.as_str() {
        "snapshot" => json!(f.store.snapshot(task.workspace_id).await?),
        "task" => json!(task),
        "claim" => {
            let entry: Entry = serde_json::from_value(d["entry"].clone())
                .map_err(|e| Error::Invalid(e.to_string()))?;
            if entry.id != command.agent.id
                || entry.version != command.agent.version
                || entry.kind != "agent"
            {
                return Err(Error::Unauthorized);
            }
            crate::registry::validate(&entry)?;
            // The peer is authoritative for its advertised agent metadata.
            json!(
                f.store
                    .claim(
                        task.id,
                        d["revision"]
                            .as_i64()
                            .ok_or_else(|| Error::Invalid("revision required".into()))?,
                        &owner,
                        &entry
                    )
                    .await?
            )
        }
        "transition" => json!(
            f.store
                .transition(
                    task.id,
                    d["revision"]
                        .as_i64()
                        .ok_or_else(|| Error::Invalid("revision required".into()))?,
                    &owner,
                    required(d, "status")?
                )
                .await?
        ),
        "complete" => json!(
            f.store
                .complete(
                    task.id,
                    &owner,
                    &key()?,
                    &serde_json::from_value::<ArtifactInput>(d["artifact"].clone())?
                )
                .await?
        ),
        "artifact" => json!(
            f.store
                .publish_artifact(
                    task.id,
                    &owner,
                    &key()?,
                    &serde_json::from_value::<ArtifactInput>(d["artifact"].clone())?
                )
                .await?
        ),
        "create_task" => json!(
            f.store
                .create_task(
                    task.workspace_id,
                    &serde_json::from_value::<NewTask>(d["task"].clone())?,
                    &owner,
                    Some(&key()?)
                )
                .await?
        ),
        "delegate" => {
            let child_id = required(d, "task_id")?
                .parse()
                .map_err(|_| Error::Invalid("invalid task ID".into()))?;
            let child = f.store.task(child_id).await?;
            if child.workspace_id != task.workspace_id {
                return Err(Error::Unauthorized);
            }
            json!(
                f.delegate(
                    child_id,
                    required(d, "node_id")?,
                    &serde_json::from_value::<EntityRef>(d["agent"].clone())?
                )
                .await?
            )
        }
        "message" | "human_message" => {
            let sender = if command.operation == "human_message" {
                format!("human@{node}")
            } else {
                owner.clone()
            };
            f.store
                .message(
                    task.workspace_id,
                    &sender,
                    required(d, "content")?,
                    Some(&key()?),
                )
                .await?;
            json!({"sent":true})
        }
        "event" => {
            let event_id =
                crate::registry::digest(&json!({"node":node,"task":task.id,"key":key()?}));
            let mut tx = f.store.pool.begin().await?;
            // A deterministic UUID-sized identifier is enough for the inbox key;
            // the full source namespace is included in the hash.
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(event_id.as_bytes());
            let mut bytes = [0; 16];
            bytes.copy_from_slice(&hash[..16]);
            let id = Uuid::from_bytes(bytes);
            let inserted = sqlx::query(
                "INSERT INTO peer_events(node_id,event_id) VALUES($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(node)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if inserted > 0 {
                f.store.event(&mut tx,Some(task.workspace_id),"federation.event",json!({"node_id":node,"task_id":task.id,"kind":required(d,"kind")?,"data":d["data"]})).await?;
            }
            tx.commit().await?;
            json!({"received":true})
        }
        _ => return Err(Error::Invalid("unknown federation operation".into())),
    };
    Ok(Json(result))
}
async fn peer_observe(State(f): State<Federation>, headers: HeaderMap) -> Result<Json<Value>> {
    let node = peer_node(&headers)?;
    let runs: Vec<Run> =
        sqlx::query_as("SELECT * FROM runs WHERE home_node=$1 ORDER BY updated_at DESC LIMIT 500")
            .bind(node)
            .fetch_all(&f.store.pool)
            .await?;
    let requests:Vec<HumanRequest>=sqlx::query_as("SELECT h.* FROM human_requests h JOIN runs r ON r.id=h.run_id WHERE r.home_node=$1 ORDER BY h.created_at DESC LIMIT 500").bind(node).fetch_all(&f.store.pool).await?;
    let invocations:Vec<Invocation>=sqlx::query_as("SELECT i.* FROM invocations i JOIN runs r ON r.id=i.run_id WHERE r.home_node=$1 ORDER BY i.created_at DESC LIMIT 500").bind(node).fetch_all(&f.store.pool).await?;
    Ok(Json(
        json!({"node_id":f.config.node_id,"runs":runs,"human_requests":requests,"invocations":invocations}),
    ))
}
#[derive(Deserialize)]
struct RemoteControl {
    run_id: Uuid,
    action: String,
    request_id: Option<Uuid>,
    response: Option<Value>,
    content: Option<String>,
}
async fn peer_control(
    State(f): State<Federation>,
    headers: HeaderMap,
    Json(input): Json<RemoteControl>,
) -> Result<Json<Value>> {
    let node = peer_node(&headers)?;
    let run = f.store.run(input.run_id).await?;
    if run.home_node != node {
        return Err(Error::Unauthorized);
    }
    match input.action.as_str() {
        "answer" => {
            let id = input
                .request_id
                .ok_or_else(|| Error::Invalid("request_id required".into()))?;
            let valid: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM human_requests WHERE id=$1 AND run_id=$2)",
            )
            .bind(id)
            .bind(run.id)
            .fetch_one(&f.store.pool)
            .await?;
            if !valid {
                return Err(Error::Unauthorized);
            }
            Ok(Json(json!(
                f.store
                    .answer(
                        id,
                        input
                            .response
                            .ok_or_else(|| Error::Invalid("response required".into()))?
                    )
                    .await?
            )))
        }
        "message" => {
            let content = input
                .content
                .ok_or_else(|| Error::Invalid("content required".into()))?;
            let home = crate::federation::Home { federation: f, run };
            home.human_message(&format!("human:{}", Uuid::new_v4()), &content)
                .await?;
            Ok(Json(json!({"sent":true})))
        }
        action => Ok(Json(json!(f.store.control(run.id, action).await?))),
    }
}
async fn mesh(State(f): State<Federation>) -> Result<Json<Value>> {
    let mut nodes = Vec::new();
    let mut errors = Vec::new();
    for peer in f.peers().await?.into_iter().filter(|p| p.enabled) {
        match f
            .request::<Value>(&peer.node_id, reqwest::Method::GET, "/observe", None)
            .await
        {
            Ok(data) => nodes.push(data),
            Err(e) => errors.push(json!({"node_id":peer.node_id,"error":e.to_string()})),
        }
    }
    Ok(Json(json!({"nodes":nodes,"errors":errors})))
}
async fn remote_action(
    State(f): State<Federation>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    Ok(Json(
        f.request(
            required(&input, "node_id")?,
            reqwest::Method::POST,
            "/control",
            Some(&input["control"]),
        )
        .await?,
    ))
}
