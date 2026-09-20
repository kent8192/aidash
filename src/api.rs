use crate::{
    Error, Result,
    api_schema::*,
    authorization::{
        Authorization, catalog, execution, identity::Actor, interaction, workspace::Workspaces,
    },
    config::{PROTOCOL_VERSION, same_secret},
    domain::*,
    federation::{Delegation, Discovery, Federation, Offer, Peer},
    registry::{EntityRef, Entry, Package, PackageRecord, Search},
    store::Invocation,
    tool::required,
};
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, Request, State},
    http::HeaderMap,
    middleware::{self, Next},
    response::{
        Response, Sse,
        sse::{Event as SseEvent, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

fn ordinary_routes() -> OpenApiRouter<Federation> {
    let administration = OpenApiRouter::new()
        .merge(crate::authorization::api::routes())
        .routes(routes!(registry_create))
        .routes(routes!(peer_create))
        .routes(routes!(mesh))
        .routes(routes!(remote_action))
        .routes(routes!(marketplace))
        .routes(routes!(package_publish))
        .routes(routes!(package_install))
        .route_layer(middleware::from_fn(operator_only));
    OpenApiRouter::new()
        .merge(administration)
        .merge(crate::generation::api::routes())
        .merge(crate::semantic::api::routes())
        .routes(routes!(human_answer))
        .routes(routes!(run_message))
        .routes(routes!(conversation_create))
        .routes(routes!(task_abandon))
        .routes(routes!(discover))
        .routes(routes!(run_control))
        .routes(routes!(run_get))
        .routes(routes!(task_delegate))
        .routes(routes!(task_claim))
        .routes(routes!(registry_get))
        .routes(routes!(registry_list))
        .routes(routes!(state))
        .routes(routes!(workspace_create))
        .routes(routes!(workspace_get))
        .routes(routes!(workspace_update))
        .routes(routes!(task_create))
        .routes(routes!(message_create))
        .routes(routes!(events))
        .routes(routes!(stream))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    let (_, mut document) = OpenApiRouter::<Federation>::new()
        .nest(
            "/api",
            ordinary_routes()
                .merge(crate::transactions::api::routes())
                .merge(crate::orchestration::routes())
                .routes(routes!(session)),
        )
        .split_for_parts();
    document.info = utoipa::openapi::Info::new("Aidash API", env!("CARGO_PKG_VERSION"));
    document.info.description = Some("Management API for the Aidash agent mesh.".into());
    document
        .components
        .get_or_insert_with(Default::default)
        .add_security_scheme(
            "bearer_auth",
            utoipa::openapi::security::SecurityScheme::Http(utoipa::openapi::security::Http::new(
                utoipa::openapi::security::HttpAuthScheme::Bearer,
            )),
        );
    document
}

pub fn router(f: Federation) -> Router {
    let (api, _) = ordinary_routes().split_for_parts();
    let (transactions, _) = crate::transactions::api::routes()
        .merge(crate::orchestration::routes())
        .routes(routes!(session))
        .split_for_parts();
    let api = api
        .route_layer(middleware::from_fn_with_state(f.clone(), node_visibility))
        .merge(transactions);
    let api = api.route_layer(middleware::from_fn_with_state(f.clone(), api_auth));
    let federation = Router::new()
        .route("/discover", post(peer_discover))
        .route("/offers", post(peer_offer))
        .route("/workspace", post(peer_workspace))
        .route("/observe", get(peer_observe))
        .route("/control", post(peer_control))
        .route_layer(middleware::from_fn_with_state(f.clone(), node_visibility))
        .merge(crate::transactions::api::peer_routes())
        .route_layer(middleware::from_fn_with_state(f.clone(), peer_auth));
    let web = tower_http::services::ServeDir::new(&f.config.web_dir).not_found_service(
        tower_http::services::ServeFile::new(format!("{}/index.html", f.config.web_dir)),
    );
    Router::new()
        .route("/health", get(health))
        .route("/api/openapi.json", get(|| async { Json(openapi()) }))
        .route(
            "/.well-known/aidash",
            get(identity).layer(middleware::from_fn_with_state(f.clone(), node_visibility)),
        )
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

#[utoipa::path(get,path="/session",operation_id="session",responses((status=200,body=SessionResponse)),security(("bearer_auth"=[])))]
async fn session(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
) -> Json<SessionResponse> {
    let access = match actor {
        Actor::Operator => AccessProfile::Operator,
        Actor::Subject(identity) => AccessProfile::Subject {
            tenant: identity.tenant,
            subject: identity.subject,
        },
    };
    Json(SessionResponse {
        access,
        node_id: f.config.node_id,
    })
}
async fn node_visibility(
    State(f): State<Federation>,
    request: Request,
    next: Next,
) -> Result<Response> {
    let _visibility = crate::transactions::gate::ReadLease::begin(&f.store).await?;
    Ok(next.run(request).await)
}
async fn api_auth(
    State(f): State<Federation>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let token = bearer(request.headers()).ok_or(Error::Unauthorized)?;
    let actor = if same_secret(token, &f.config.api_token) {
        Actor::Operator
    } else {
        Authorization {
            pool: f.store.pool.clone(),
        }
        .authenticate(token)
        .await?
    };
    request.extensions_mut().insert(actor);
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
pub(crate) async fn operator_only(request: Request, next: Next) -> Result<Response> {
    if !matches!(request.extensions().get::<Actor>(), Some(Actor::Operator)) {
        return Err(Error::Forbidden);
    }
    Ok(next.run(request).await)
}
fn scoped(f: &Federation, actor: Actor) -> Option<Workspaces> {
    match actor {
        Actor::Operator => None,
        Actor::Subject(identity) => Some(Workspaces {
            store: f.store.clone(),
            identity,
        }),
    }
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
    f.authenticate_peer(node, bearer(headers).ok_or(Error::Unauthorized)?)
        .await?;
    Ok(next.run(request).await)
}
pub(crate) fn peer_node(headers: &HeaderMap) -> Result<&str> {
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
#[utoipa::path(get, path = "/state", operation_id = "state", responses((status = 200, body = StateResponse)), security(("bearer_auth" = [])))]
async fn state(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
) -> Result<Json<StateResponse>> {
    if let Some(scope) = scoped(&f, actor) {
        return Ok(Json(scope.state(f.config.identity(vec![])).await?));
    }
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
    let installations: Vec<Installation> =
        sqlx::query_as("SELECT * FROM installations ORDER BY installed_at DESC")
            .fetch_all(&f.store.pool)
            .await?;
    Ok(Json(StateResponse {
        access: AccessProfile::Operator,
        node: f.config.identity(vec![]),
        registry: records,
        workspaces: f.store.workspaces().await?,
        tasks: f.store.tasks(None).await?,
        runs: f.store.runs().await?,
        human_requests: human,
        conversations,
        peers: f.peers().await?,
        events,
        artifacts,
        installations,
    }))
}
#[utoipa::path(get, path = "/registry", operation_id = "registry_list", params(Search), responses((status = 200, body = [Entry])), security(("bearer_auth" = [])))]
async fn registry_list(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Query(search): Query<Search>,
) -> Result<Json<Vec<Entry>>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(catalog::list(&f.store, &identity, &search).await?));
    }
    Ok(Json(f.registry.list(&search).await?))
}
#[utoipa::path(get, path = "/registry/{id}/{version}", operation_id = "registry_get", params(("id" = String, Path),("version" = String, Path)), responses((status = 200, body = Entry)), security(("bearer_auth" = [])))]
async fn registry_get(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<Entry>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(
            catalog::get(&f.store, &identity, &EntityRef { id, version }).await?,
        ));
    }
    Ok(Json(f.registry.get(&id, &version).await?))
}
#[utoipa::path(post, path = "/registry", operation_id = "registry_create", request_body = Entry, responses((status = 200, body = Entry)), security(("bearer_auth" = [])))]
async fn registry_create(
    State(f): State<Federation>,
    Json(entry): Json<Entry>,
) -> Result<Json<Entry>> {
    let mut tx = f.store.pool.begin().await?;
    crate::registry::register_in(&mut tx, &entry).await?;
    f.store
        .event(
            &mut tx,
            None,
            "registry.registered",
            json!({"id":entry.id,"version":entry.version,"kind":entry.kind}),
        )
        .await?;
    tx.commit().await?;
    Ok(Json(entry))
}
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceInput {
    title: String,
    goal: String,
}
#[utoipa::path(post, path = "/workspaces", operation_id = "workspace_create", request_body = WorkspaceInput, responses((status = 200, body = Workspace)), security(("bearer_auth" = [])))]
async fn workspace_create(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Json(input): Json<WorkspaceInput>,
) -> Result<Json<Workspace>> {
    if let Some(scope) = scoped(&f, actor) {
        return Ok(Json(scope.create(&input.title, &input.goal).await?));
    }
    Ok(Json(
        f.store.create_workspace(&input.title, &input.goal).await?,
    ))
}
#[utoipa::path(get, path = "/workspaces/{id}", operation_id = "workspace_get", params(("id" = Uuid, Path)), responses((status = 200, body = WorkspaceSnapshot)), security(("bearer_auth" = [])))]
async fn workspace_get(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceSnapshot>> {
    if let Some(scope) = scoped(&f, actor) {
        return Ok(Json(scope.snapshot(id).await?));
    }
    Ok(Json(f.store.snapshot(id).await?))
}
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct StateInput {
    revision: i64,
    state: Value,
}
#[utoipa::path(patch, path = "/workspaces/{id}", operation_id = "workspace_update", request_body = StateInput, params(("id" = Uuid, Path)), responses((status = 200, body = Workspace)), security(("bearer_auth" = [])))]
async fn workspace_update(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(input): Json<StateInput>,
) -> Result<Json<Workspace>> {
    if let Some(scope) = scoped(&f, actor) {
        return Ok(Json(scope.update(id, input.revision, input.state).await?));
    }
    Ok(Json(
        f.store
            .update_state(id, input.revision, input.state)
            .await?,
    ))
}
#[utoipa::path(post, path = "/workspaces/{id}/tasks", operation_id = "task_create", request_body = NewTask, params(("id" = Uuid, Path)), responses((status = 200, body = Task)), security(("bearer_auth" = [])))]
async fn task_create(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<NewTask>,
) -> Result<Json<Task>> {
    if let Some(scope) = scoped(&f, actor) {
        return Ok(Json(
            scope
                .create_task(
                    id,
                    &input,
                    headers.get("idempotency-key").and_then(|h| h.to_str().ok()),
                )
                .await?,
        ));
    }
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
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct MessageInput {
    content: String,
}
#[utoipa::path(post, path = "/workspaces/{id}/messages", operation_id = "message_create", request_body = MessageInput, params(("id" = Uuid, Path)), responses((status = 200, body = SentResponse)), security(("bearer_auth" = [])))]
async fn message_create(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(input): Json<MessageInput>,
) -> Result<Json<SentResponse>> {
    if let Some(scope) = scoped(&f, actor) {
        scope.message(id, &input.content).await?;
        return Ok(Json(SentResponse { sent: true }));
    }
    f.store.message(id, "human", &input.content, None).await?;
    Ok(Json(SentResponse { sent: true }))
}
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
struct ClaimInput {
    revision: i64,
    agent: EntityRef,
}
#[utoipa::path(post, path = "/tasks/{id}/claim", operation_id = "task_claim", request_body = ClaimInput, params(("id" = Uuid, Path)), responses((status = 200, body = Task)), security(("bearer_auth" = [])))]
async fn task_claim(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(input): Json<ClaimInput>,
) -> Result<Json<Task>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(
            execution::claim(&f, &identity, id, input.revision, &input.agent).await?,
        ));
    }
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
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
struct DelegateInput {
    node_id: String,
    agent: EntityRef,
}
#[utoipa::path(post, path = "/tasks/{id}/delegate", operation_id = "task_delegate", request_body = DelegateInput, params(("id" = Uuid, Path)), responses((status = 200, body = Delegation)), security(("bearer_auth" = [])))]
async fn task_delegate(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(input): Json<DelegateInput>,
) -> Result<Json<Delegation>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(
            execution::delegate(&f, &identity, id, &input.node_id, &input.agent).await?,
        ));
    }
    Ok(Json(f.delegate(id, &input.node_id, &input.agent).await?))
}
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
struct AbandonInput {
    revision: i64,
    reason: String,
}
#[utoipa::path(post, path = "/tasks/{id}/abandon", operation_id = "task_abandon", request_body = AbandonInput, params(("id" = Uuid, Path)), responses((status = 200, body = Task)), security(("bearer_auth" = [])))]
async fn task_abandon(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(input): Json<AbandonInput>,
) -> Result<Json<Task>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(
            interaction::abandon(&f, &identity, id, input.revision, &input.reason).await?,
        ));
    }
    let task = f
        .store
        .abandon_task(id, input.revision, &input.reason)
        .await?;
    f.notify.notify_waiters();
    Ok(Json(task))
}
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct ConversationInput {
    title: String,
    goal: String,
    target: EntityRef,
    target_kind: String,
}
#[utoipa::path(post, path = "/conversations", operation_id = "conversation_create", request_body = ConversationInput, responses((status = 200, body = ConversationResponse)), security(("bearer_auth" = [])))]
async fn conversation_create(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Json(input): Json<ConversationInput>,
) -> Result<Json<ConversationResponse>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(
            interaction::conversation(
                &f,
                &identity,
                &input.title,
                &input.goal,
                &input.target,
                &input.target_kind,
            )
            .await?,
        ));
    }
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
    f.store
        .require_legacy_agent(&agent.id, &agent.version)
        .await?;
    let mut tx = f.store.pool.begin().await?;
    let workspace = f
        .store
        .create_workspace_in(&mut tx, Uuid::new_v4(), &input.title, &input.goal)
        .await?;
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
    f.store
        .message_in(&mut tx, workspace.id, "human", &input.goal, None)
        .await?;
    let task = f
        .store
        .create_task_in(
            &mut tx,
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
    let delegation = f
        .delegate_in(&mut tx, &task, &f.config.node_id, &agent)
        .await?;
    tx.commit().await?;
    if let Err(error) = f.deliver(&delegation).await {
        tracing::warn!(%error, "conversation execution queued for retry");
    }
    Ok(Json(ConversationResponse {
        conversation: c,
        workspace,
        task,
        delegation,
    }))
}
#[utoipa::path(get, path = "/runs/{id}", operation_id = "run_get", params(("id" = Uuid, Path)), responses((status = 200, body = RunDetails)), security(("bearer_auth" = [])))]
async fn run_get(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunDetails>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(execution::details(&f, &identity, id).await?));
    }
    let run = f.store.run(id).await?;
    let invocations: Vec<Invocation> =
        sqlx::query_as("SELECT * FROM invocations WHERE run_id=$1 ORDER BY created_at")
            .bind(id)
            .fetch_all(&f.store.pool)
            .await?;
    Ok(Json(RunDetails {
        memory: f.store.memory(&run).await?,
        run,
        invocations,
    }))
}
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
struct ControlInput {
    action: String,
}
#[utoipa::path(post, path = "/runs/{id}/control", operation_id = "run_control", request_body = ControlInput, params(("id" = Uuid, Path)), responses((status = 200, body = Run)), security(("bearer_auth" = [])))]
async fn run_control(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(input): Json<ControlInput>,
) -> Result<Json<Run>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(
            execution::control(&f, &identity, id, &input.action).await?,
        ));
    }
    let r = f.store.control(id, &input.action).await?;
    f.notify.notify_waiters();
    Ok(Json(r))
}
#[utoipa::path(post, path = "/runs/{id}/message", operation_id = "run_message", request_body = MessageInput, params(("id" = Uuid, Path)), responses((status = 200, body = SentResponse)), security(("bearer_auth" = [])))]
async fn run_message(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(input): Json<MessageInput>,
) -> Result<Json<SentResponse>> {
    if let Actor::Subject(identity) = actor {
        interaction::message(&f, &identity, id, &input.content).await?;
        return Ok(Json(SentResponse { sent: true }));
    }
    let run = f.store.run(id).await?;
    let home = crate::federation::Home::new(f, run);
    home.human_message(&format!("human:{}", Uuid::new_v4()), &input.content)
        .await?;
    Ok(Json(SentResponse { sent: true }))
}
#[utoipa::path(post, path = "/human-requests/{id}/answer", operation_id = "human_answer", request_body = Value, params(("id" = Uuid, Path)), responses((status = 200, body = HumanRequest)), security(("bearer_auth" = [])))]
async fn human_answer(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    Json(response): Json<Value>,
) -> Result<Json<HumanRequest>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(
            interaction::answer(&f, &identity, id, response).await?,
        ));
    }
    let h = f.store.answer(id, response).await?;
    f.notify.notify_waiters();
    Ok(Json(h))
}
#[utoipa::path(post, path = "/peers", operation_id = "peer_create", request_body = Peer, responses((status = 200, body = Peer)), security(("bearer_auth" = [])))]
async fn peer_create(State(f): State<Federation>, Json(peer): Json<Peer>) -> Result<Json<Peer>> {
    Ok(Json(f.register_peer(peer).await?))
}
#[utoipa::path(post, path = "/discover", operation_id = "discover", request_body = Search, responses((status = 200, body = Discovery)), security(("bearer_auth" = [])))]
async fn discover(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Json(query): Json<Search>,
) -> Result<Json<Discovery>> {
    if let Actor::Subject(identity) = actor {
        return Ok(Json(execution::discover(&f, &identity, &query).await?));
    }
    Ok(Json(f.discover(&query).await?))
}
#[utoipa::path(get, path = "/marketplace", operation_id = "marketplace", params(Search), responses((status = 200, body = [PackageRecord])), security(("bearer_auth" = [])))]
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
#[utoipa::path(post, path = "/marketplace", operation_id = "package_publish", request_body = Package, responses((status = 200, body = PackageRecord)), security(("bearer_auth" = [])))]
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
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
struct InstallInput {
    digest: String,
    #[serde(default = "empty_object")]
    config: Value,
}
#[utoipa::path(post, path = "/marketplace/{id}/{version}/install", operation_id = "package_install", request_body = InstallInput, params(("id" = String, Path),("version" = String, Path)), responses((status = 200, body = Entry)), security(("bearer_auth" = [])))]
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
#[derive(Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct EventQuery {
    #[serde(default)]
    after: i64,
    workspace_id: Option<Uuid>,
}
#[utoipa::path(get, path = "/events", operation_id = "events", params(EventQuery), responses((status = 200, body = [crate::domain::Event])), security(("bearer_auth" = [])))]
async fn events(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Query(q): Query<EventQuery>,
) -> Result<Json<Vec<crate::domain::Event>>> {
    if let Some(scope) = scoped(&f, actor) {
        return Ok(Json(scope.events(q.after, q.workspace_id, 500).await?));
    }
    Ok(Json(f.store.events(q.after, q.workspace_id, 500).await?))
}
#[utoipa::path(get, path = "/events/stream", operation_id = "stream", params(EventQuery), responses((status = 200, body = String, content_type = "text/event-stream")), security(("bearer_auth" = [])))]
async fn stream(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Query(q): Query<EventQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = std::result::Result<SseEvent, Infallible>>>> {
    let scope = scoped(&f, actor);
    let mut cursor = headers
        .get("last-event-id")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(q.after);
    // Validate the requested workspace before committing the SSE response.
    if let Some(scope) = &scope {
        scope.events(cursor, q.workspace_id, 1).await?;
    }
    let stream = async_stream::stream! {
        loop {
            let visibility=match crate::transactions::gate::ReadLease::begin(&f.store).await {
                Ok(lease)=>lease,
                Err(Error::TransactionPending)=>{tokio::time::sleep(Duration::from_millis(250)).await;continue;},
                Err(_)=>{yield Ok(SseEvent::default().event("error").data("event stream interrupted"));return;}
            };
            let events = if let Some(scope) = &scope { scope.poll_events(cursor, q.workspace_id, 100).await }
                else { f.store.events(cursor, q.workspace_id, 100).await.map(|events| { let scanned = events.last().map_or(cursor, |event| event.sequence); (events, scanned) }) };
            drop(visibility);
            match events {
                Ok((events, scanned)) => { for event in events {
                    let visibility=loop {
                        match crate::transactions::gate::ReadLease::begin(&f.store).await {
                            Ok(lease)=>break lease,
                            Err(Error::TransactionPending)=>tokio::time::sleep(Duration::from_millis(250)).await,
                            Err(_)=>{yield Ok(SseEvent::default().event("error").data("event stream interrupted"));return;}
                        }
                    };
                    cursor = event.sequence;
                    if let Some(scope) = &scope {
                        match scope.can_emit(&event).await {
                            Ok(true) => {},
                            Ok(false) => continue,
                            Err(_) => { yield Ok(SseEvent::default().event("error").data("event stream interrupted")); return; }
                        }
                    }
                    drop(visibility);
                    yield Ok(SseEvent::default().id(cursor.to_string()).event("mesh").data(event.cloud_event().to_string()));
                } cursor = scanned; },
                Err(e) => { tracing::error!(error=%e,"SSE read failed"); yield Ok(SseEvent::default().event("error").data("event stream interrupted")); break; }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
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
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
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
    if !matches!(
        command.operation.as_str(),
        "snapshot" | "snapshot_workspace" | "snapshot_page" | "task" | "claim"
    ) && !(task.owner.is_none()
        && (command.operation == "human_message"
            || (command.operation == "transition"
                && (d["status"] == "CANCELLED" || d["status"] == "FAILED"))))
        && task.owner.as_deref() != Some(&owner)
    {
        return Err(Error::Unauthorized);
    }
    if matches!(
        task.status.as_str(),
        "COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
    ) {
        let read = matches!(
            command.operation.as_str(),
            "snapshot" | "snapshot_workspace" | "snapshot_page" | "task"
        );
        let replay_completion = command.operation == "complete" && task.status == "COMPLETED";
        let replay_transition = command.operation == "transition" && d["status"] == task.status;
        if !read && !replay_completion && !replay_transition {
            return Err(Error::Unauthorized);
        }
    }
    let result = match command.operation.as_str() {
        "snapshot" => json!(f.store.snapshot(task.workspace_id).await?),
        "snapshot_workspace" => json!(f.store.workspace(task.workspace_id).await?),
        "snapshot_page" => json!(
            f.store
                .snapshot_page(
                    task.workspace_id,
                    required(d, "collection")?,
                    serde_json::from_value(d["after"].clone())?
                )
                .await?
        ),
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
#[derive(Deserialize, Serialize, utoipa::ToSchema)]
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
            let home = crate::federation::Home::new(f, run);
            home.human_message(&format!("human:{}", Uuid::new_v4()), &content)
                .await?;
            Ok(Json(json!({"sent":true})))
        }
        action => Ok(Json(json!(f.store.control(run.id, action).await?))),
    }
}
#[utoipa::path(get, path = "/mesh", operation_id = "mesh", responses((status = 200, body = MeshResponse)), security(("bearer_auth" = [])))]
async fn mesh(State(f): State<Federation>) -> Result<Json<MeshResponse>> {
    let mut nodes = Vec::new();
    let mut errors = Vec::new();
    let peers = f.peers().await?.into_iter().filter(|p| p.enabled);
    let f = &f;
    let mut responses = stream::iter(peers.map(|peer| async move {
        let response = f
            .request::<MeshNode>(&peer.node_id, reqwest::Method::GET, "/observe", None)
            .await;
        (peer, response)
    }))
    .buffer_unordered(8);
    while let Some((peer, response)) = responses.next().await {
        match response {
            Ok(data) => nodes.push(data),
            Err(e) => errors.push(PeerError {
                node_id: peer.node_id,
                error: e.to_string(),
            }),
        }
    }
    Ok(Json(MeshResponse { nodes, errors }))
}
#[utoipa::path(post, path = "/remote", operation_id = "remote_action", request_body = RemoteActionInput, responses((status = 200, body = Value)), security(("bearer_auth" = [])))]
async fn remote_action(
    State(f): State<Federation>,
    Json(input): Json<RemoteActionInput>,
) -> Result<Json<Value>> {
    Ok(Json(
        f.request(
            &input.node_id,
            reqwest::Method::POST,
            "/control",
            Some(&serde_json::to_value(input.control)?),
        )
        .await?,
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RemoteActionInput {
    node_id: String,
    control: RemoteControl,
}

#[cfg(test)]
mod schema_tests {
    #[test]
    fn openapi_describes_authenticated_management_routes_and_streams() {
        // The router itself registers these operations with utoipa-axum, so an
        // export needs neither environment configuration nor a live database.
        let document = serde_json::to_value(super::openapi()).unwrap();
        assert!(document["components"]["schemas"]["Policy"]["properties"]["actions"].is_object());
        assert!(
            document["components"]["schemas"]["GenerationPolicy"]["properties"]["spec"].is_object()
        );
        let paths = document["paths"].as_object().unwrap();
        assert_eq!(
            paths
                .values()
                .map(|path| path.as_object().unwrap().len())
                .sum::<usize>(),
            63
        );
        for (path, operations) in paths {
            assert!(path.starts_with("/api/"));
            for operation in operations.as_object().unwrap().values() {
                assert!(operation["operationId"].is_string());
                assert_eq!(
                    operation["security"][0]["bearer_auth"],
                    serde_json::json!([])
                );
            }
        }
        assert!(document["paths"]["/api/events/stream"]["get"]["responses"]["200"]["content"]["text/event-stream"].is_object());
        assert!(
            document["components"]["schemas"]["StateResponse"]["properties"]["tasks"].is_object()
        );
        assert_eq!(
            document["components"]["schemas"]["Search"]["additionalProperties"],
            false
        );
    }
}
