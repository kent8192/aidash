use super::{contracts::*, management::*, service, sessions};
use crate::{
	Error, Result,
	authorization::{access::Access, execution, identity::Actor},
	domain::{NewTask, Run},
	federation::Federation,
	registry::EntityRef,
};
use axum::{
	Extension, Json,
	extract::{Path, Query as QueryInput, State},
};
use sea_orm::sea_query::{Alias, Expr, Order, PostgresQueryBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(areas))
		.routes(routes!(configure_agent))
		.routes(routes!(operation_history))
		.routes(routes!(reference_list))
		.routes(routes!(area_for_run))
		.routes(routes!(file_read))
		.routes(routes!(file_search))
		.routes(routes!(materialize))
		.routes(routes!(session))
		.routes(routes!(enqueue))
		.routes(routes!(thread_run))
		.routes(routes!(thread_delete))
		.routes(routes!(steer))
		.routes(routes!(shell))
		.routes(routes!(shell_poll))
		.routes(routes!(shell_cancel))
		.routes(routes!(apply_patch))
		.routes(routes!(file_share))
		.routes(routes!(transfer_status))
		.routes(routes!(transfer_history))
		.routes(routes!(transfer_reconcile))
		.routes(routes!(transfer_recipients))
		.routes(routes!(outbound_get))
		.routes(routes!(outbound_history))
		.routes(routes!(python))
		.routes(routes!(python_install))
		.routes(routes!(python_poll))
		.routes(routes!(python_cancel))
		.routes(routes!(approval_list))
		.routes(routes!(approval_decide))
		.routes(routes!(approval_revoke))
		.routes(routes!(grant_revoke))
		.routes(routes!(skill_import))
		.routes(routes!(skill_list))
		.routes(routes!(skill_load))
		.routes(routes!(skill_read))
		.routes(routes!(reference_upload))
		.routes(routes!(reference_chunk))
		.routes(routes!(reference_commit))
		.routes(routes!(reference_inspect))
		.routes(routes!(reference_revoke))
		.routes(routes!(reference_download))
		.routes(routes!(file_download))
		.routes(routes!(managed_areas))
		.routes(routes!(cleanup))
		.routes(routes!(cleanup_status))
		.routes(routes!(cleanup_reconcile))
		.routes(routes!(deletion_confirmation))
		.routes(routes!(restore))
		.layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024))
		.layer(axum::middleware::from_fn(super::errors::http))
}
pub(crate) async fn access(f: &Federation, actor: Actor) -> Result<Access> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Invalid(
			"core file operations require a tenant subject credential".into(),
		));
	};
	Access::begin(&f.store, &identity).await
}
fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<Json<T>> {
	Ok(Json(serde_json::from_value(value)?))
}
#[utoipa::path(post,path="/agents/{id}/capabilities",operation_id="core_agent_configure",params(("id"=String,Path)),request_body=super::configuration::Configure,responses((status=200,body=ConfiguredAgent)),security(("bearer_auth"=[])))]
async fn configure_agent(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<String>,
	Json(input): Json<super::configuration::Configure>,
) -> Result<Json<ConfiguredAgent>> {
	let mut access = access(&f, actor).await?;
	let result = super::configuration::configure(&f.store, &mut access, id, input).await;
	typed(access.finish(result).await?)
}
async fn run_access(f: &Federation, actor: Actor, id: Uuid) -> Result<(Access, Run)> {
	let mut access = access(f, actor).await?;
	let run = access.run_for_interaction(id).await?;
	execution::inherit_run_authority(&mut access, &run).await?;
	let config = service::settings(&mut access, &run).await?;
	if config.core_capabilities.enabled() {
		sessions::initialize(&f.store, &mut access, &run, &config).await?;
	}
	Ok((access, run))
}
#[derive(Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
struct Page {
	cursor: Option<Uuid>,
}
#[derive(Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
struct AreaQuery {
	cursor: Option<Uuid>,
	thread_id: Option<Uuid>,
	workspace_id: Option<Uuid>,
}
#[utoipa::path(get,path="/working-areas",operation_id="working_area_list",params(AreaQuery),responses((status=200,body=AreaPage)),security(("bearer_auth"=[])))]
async fn areas(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	QueryInput(page): QueryInput<AreaQuery>,
) -> Result<Json<AreaPage>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let rows: Vec<Area> = sqlx::query_as(
			&sessions::select("core_areas")
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$3")))
				.and_where(Expr::cust(
					"($4::uuid IS NULL OR thread_id = $4) AND ($5::uuid IS NULL OR workspace_id = $5)",
				))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(51)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.bind(&access.identity.subject)
		.bind(page.cursor.unwrap_or(Uuid::nil()))
		.bind(page.thread_id)
		.bind(page.workspace_id)
		.fetch_all(&mut **access.tx)
		.await?;
		let next_cursor = if rows.len() == 51 {
			Some(rows[49].id)
		} else {
			None
		};
		let mut items = vec![];
		for area in rows.into_iter().take(50) {
			match sessions::authorize(&mut access, &area, "file.read").await {
				Ok(()) => items.push(area),
				Err(Error::Forbidden | Error::NotFound(_)) => {}
				Err(error) => return Err(error),
			}
		}
		Ok(AreaPage { items, next_cursor })
	}
	.await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(get,path="/runs/{id}/working-area",operation_id="run_working_area",params(("id"=Uuid,Path)),responses((status=200,body=Area)),security(("bearer_auth"=[])))]
async fn area_for_run(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Area>> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = sessions::for_run(&mut access, &run).await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(post,path="/runs/{id}/files/read",operation_id="core_file_read",params(("id"=Uuid,Path)),request_body=FileRead,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn file_read(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<FileRead>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "file_read", json!(input)).await
}
#[utoipa::path(post,path="/runs/{id}/files/search",operation_id="core_file_search",params(("id"=Uuid,Path)),request_body=FileSearch,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn file_search(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<FileSearch>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "file_search", json!(input)).await
}
async fn dispatch(
	f: Federation,
	actor: Actor,
	id: Uuid,
	name: &str,
	input: Value,
) -> Result<Json<Envelope>> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = service::invoke(
		&f.store,
		&mut access,
		&run,
		name,
		input,
		&Uuid::new_v4().to_string(),
	)
	.await;
	access.finish(result).await.map(Json)
}

#[utoipa::path(post,path="/runs/{id}/shell",operation_id="core_shell",params(("id"=Uuid,Path)),request_body=Shell,responses((status=200,body=OperationResult)),security(("bearer_auth"=[])))]
async fn shell(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<Shell>,
) -> Result<Json<OperationResult>> {
	let value = dispatch(f, actor, id, "shell", json!(input)).await?;
	Ok(Json(serde_json::from_value(json!(value.0))?))
}
#[utoipa::path(post,path="/runs/{id}/patch",operation_id="core_apply_patch",params(("id"=Uuid,Path)),request_body=Patch,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn apply_patch(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<Patch>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "apply_patch", json!(input)).await
}
#[utoipa::path(post,path="/runs/{id}/shell/poll",operation_id="core_shell_poll",params(("id"=Uuid,Path)),request_body=OperationInput,responses((status=200,body=OperationResult)),security(("bearer_auth"=[])))]
async fn shell_poll(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> Result<Json<OperationResult>> {
	let value = dispatch(f, actor, id, "shell_poll", json!(input)).await?;
	Ok(Json(serde_json::from_value(json!(value.0))?))
}
#[utoipa::path(post,path="/runs/{id}/shell/cancel",operation_id="core_shell_cancel",params(("id"=Uuid,Path)),request_body=OperationInput,responses((status=200,body=OperationResult)),security(("bearer_auth"=[])))]
async fn shell_cancel(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> Result<Json<OperationResult>> {
	let value = dispatch(f, actor, id, "shell_cancel", json!(input)).await?;
	Ok(Json(serde_json::from_value(json!(value.0))?))
}
#[utoipa::path(post,path="/runs/{id}/files/materialize",operation_id="core_file_materialize",params(("id"=Uuid,Path)),request_body=Materialize,responses((status=200,body=MaterializedFile)),security(("bearer_auth"=[])))]
async fn materialize(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<Materialize>,
) -> Result<Json<MaterializedFile>> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = service::materialize(&f.store, &mut access, &run, input).await;
	typed(access.finish(result).await?)
}
#[utoipa::path(get,path="/working-areas/{id}/session",operation_id="core_session_status",params(("id"=Uuid,Path)),responses((status=200,body=SessionStatus)),security(("bearer_auth"=[])))]
async fn session(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<SessionStatus>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		sessions::status(&mut access, &area).await
	}
	.await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(post,path="/working-areas/{id}/queue",operation_id="core_session_enqueue",params(("id"=Uuid,Path)),request_body=Enqueue,responses((status=200,body=SessionStatus)),security(("bearer_auth"=[])))]
async fn enqueue(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<Enqueue>,
) -> Result<Json<SessionStatus>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		if !matches!(area.state.as_str(), "active" | "running") {
			return Err(Error::Conflict("AREA_UNAVAILABLE".into()));
		}
		let digest = crate::registry::digest(&json!(["enqueue", id, input]));
		if let Some(result) = sessions::cached(&mut access, input.idempotency_key, &digest).await? {
			return Ok(serde_json::from_value(result)?);
		}
		if sessions::status(&mut access, &area).await?.queue.len() >= 100 {
			return Err(Error::Conflict("QUEUE_LIMIT".into()));
		}
		let workspace = access.workspace(area.workspace_id).await?;
		access.require(&workspace, "task.create").await?;
		let creator = access.identity.subject.clone();
		let task = f
			.store
			.create_task_in(
				&mut access.tx,
				area.workspace_id,
				&NewTask {
					title: input.title,
					description: input.description,
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				&creator,
				None,
			)
			.await?;
		sessions::bind_task(&mut access, task.id, area.thread_id).await?;
		execution::delegate_in(
			&f,
			&mut access,
			task.id,
			&EntityRef {
				id: area.agent_id.clone(),
				version: input.agent_version,
			},
		)
		.await?;
		let result = sessions::status(&mut access, &area).await?;
		if result.queue.is_empty()
			|| !result
				.queue
				.iter()
				.any(|item| item.sequence == area.next_sequence)
		{
			return Err(Error::Invalid(
				"follow-up Agent version must enable core capabilities".into(),
			));
		}
		sessions::cache(&mut access, input.idempotency_key, &digest, &json!(result)).await?;
		Ok(result)
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(Json(result))
}

#[utoipa::path(post,path="/working-areas/{id}/steer",operation_id="core_session_steer",params(("id"=Uuid,Path)),request_body=Steer,responses((status=200,body=Steered)),security(("bearer_auth"=[])))]
async fn steer(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<Steer>,
) -> Result<Json<Steered>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		let digest = crate::registry::digest(&json!(["steer", id, input]));
		if let Some(result) = sessions::cached(&mut access, input.idempotency_key, &digest).await? {
			return Ok(serde_json::from_value(result)?);
		}
		if sessions::status(&mut access, &area).await?.active_run_id != Some(input.expected_run_id)
		{
			return Err(Error::Conflict(
				"ACTIVE_RUN_CHANGED: reload before steering".into(),
			));
		}
		let run = access.run_for_interaction(input.expected_run_id).await?;
		access
			.require(&access.resource("run", run.id, json!({})), "run.message")
			.await?;
		access
			.require(
				&access.resource("workspace", area.workspace_id, json!({})),
				"message.create",
			)
			.await?;
		let limit = f.run_message_limit(&run).await?;
		let sender = access.identity.subject.clone();
		f.store
			.accept_run_message_in(
				&mut access.tx,
				run.id,
				&sender,
				&input.content,
				&format!("steer:{}:{}", run.id, input.idempotency_key),
				limit,
			)
			.await?;
		let result = Steered {
			accepted_run_id: run.id,
		};
		sessions::cache(&mut access, input.idempotency_key, &digest, &json!(result)).await?;
		Ok(result)
	}
	.await;
	let result = access.finish(result).await?;
	let run = f.store.run(result.accepted_run_id).await?;
	if let Err(error) = f.deliver_run_messages(&run).await {
		tracing::warn!(run_id=%run.id, %error, "steer awaits durable delivery");
	}
	f.notify.notify_waiters();
	Ok(Json(result))
}

#[utoipa::path(post,path="/capabilities/skills/import",operation_id="core_skill_import",request_body=crate::skill_import::ImportRequest,responses((status=200,body=super::skills::SkillAttachment)),security(("bearer_auth"=[])))]
async fn skill_import(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Json(input): Json<crate::skill_import::ImportRequest>,
) -> Result<Json<super::skills::SkillAttachment>> {
	let mut access = access(&f, actor).await?;
	access
		.require(
			&access.resource("skill_attachment", "import", json!({})),
			"skill.import",
		)
		.await?;
	access.finish(Ok(())).await?;
	let result = crate::skill_import::import(input).await?;
	Ok(Json(super::skills::imported(result.selected.ok_or_else(
		|| Error::Invalid(format!("SELECT_SKILL: {}", json!(result.skills))),
	)?)?))
}
#[utoipa::path(post,path="/runs/{id}/skills/list",operation_id="core_skill_list",params(("id"=Uuid,Path)),request_body=super::skills::SkillList,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn skill_list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::skills::SkillList>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "skill_list", json!(input)).await
}
#[utoipa::path(post,path="/runs/{id}/skills/load",operation_id="core_skill_load",params(("id"=Uuid,Path)),request_body=super::skills::SkillLoad,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn skill_load(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::skills::SkillLoad>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "skill_load", json!(input)).await
}
#[utoipa::path(post,path="/runs/{id}/skills/read",operation_id="core_skill_read",params(("id"=Uuid,Path)),request_body=super::skills::SkillRead,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn skill_read(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::skills::SkillRead>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "skill_read", json!(input)).await
}

#[utoipa::path(post,path="/runs/{id}/files/share",operation_id="core_file_share",params(("id"=Uuid,Path)),request_body=super::sharing::Share,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn file_share(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::sharing::Share>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "file_share", json!(input)).await
}

#[utoipa::path(post,path="/workspaces/{workspace}/threads/{thread}/agents/{agent}/runs",operation_id="core_thread_run",params(("workspace"=Uuid,Path),("thread"=Uuid,Path),("agent"=String,Path)),request_body=Enqueue,responses((status=200,body=Area)),security(("bearer_auth"=[])))]
async fn thread_run(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((workspace, thread, agent)): Path<(Uuid, Uuid, String)>,
	Json(input): Json<Enqueue>,
) -> Result<Json<Area>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let digest =
			crate::registry::digest(&json!(["thread_run", workspace, thread, agent, input]));
		if let Some(previous) =
			sessions::cached(&mut access, input.idempotency_key, &digest).await?
		{
			let id = serde_json::from_value(previous["area_id"].clone())?;
			return sessions::load(&mut access, id).await;
		}
		super::thread_lifecycle::visible(&mut access.tx, thread).await?;
		let channel: crate::collaboration::ChannelThread = sqlx::query_as(
			&sessions::select("channel_threads")
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(thread)
		.bind(workspace)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or_else(|| Error::NotFound("thread unavailable".into()))?;
		access
			.workspace_record(workspace, "message", channel.root_message_id)
			.await?;
		let resource = access.workspace(workspace).await?;
		access.require(&resource, "task.create").await?;
		let creator = access.identity.subject.clone();
		let task = f
			.store
			.create_task_in(
				&mut access.tx,
				workspace,
				&NewTask {
					title: input.title,
					description: input.description,
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				&creator,
				None,
			)
			.await?;
		sessions::bind_task(&mut access, task.id, thread).await?;
		let delegation = execution::delegate_in(
			&f,
			&mut access,
			task.id,
			&EntityRef {
				id: agent,
				version: input.agent_version,
			},
		)
		.await?;
		let run: Run = sqlx::query_as(
			&sessions::select("runs")
				.and_where(Expr::col(Alias::new("task_id")).eq(Expr::cust("$1")))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.bind(task.id)
		.fetch_one(&mut **access.tx)
		.await?;
		let _ = delegation;
		let area = sessions::for_run(&mut access, &run).await?;
		sessions::cache(
			&mut access,
			input.idempotency_key,
			&digest,
			&json!({"area_id":area.id}),
		)
		.await?;
		Ok(area)
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(Json(result))
}

#[utoipa::path(post,path="/runs/{id}/outbound",operation_id="core_outbound_get",params(("id"=Uuid,Path)),request_body=super::approvals::Outbound,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn outbound_get(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::approvals::Outbound>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "outbound_get", json!(input)).await
}
#[derive(serde::Serialize, utoipa::ToSchema)]
struct OutboundPage {
	items: Vec<OutboundStatus>,
	next_cursor: Option<Uuid>,
}
#[utoipa::path(get,path="/runs/{id}/outbound",operation_id="core_outbound_history",params(("id"=Uuid,Path),Page),responses((status=200,body=OutboundPage)),security(("bearer_auth"=[])))]
async fn outbound_history(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	QueryInput(page): QueryInput<Page>,
) -> Result<Json<OutboundPage>> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result = async {
		sessions::context_authority(&mut access, &run).await?;
		let records: Vec<super::records::Record> = sqlx::query_as(
			&sessions::select("core_records")
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("kind")).eq("outbound"))
				.and_where(Expr::cust("data->>'run_id' = $3"))
				.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$4")))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(51)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.bind(&access.identity.subject)
		.bind(id.to_string())
		.bind(page.cursor.unwrap_or(Uuid::nil()))
		.fetch_all(&mut **access.tx)
		.await?;
		let next_cursor = (records.len() == 51).then(|| records[49].id);
		let mut items = vec![];
		for record in records.into_iter().take(50) {
			let mut item = super::approvals::view(&f.store, &mut access, &run, &record).await?;
			item["url"] = record.data["url"].clone();
			item["final_url"] = record.data["final_url"].clone();
			items.push(serde_json::from_value(item)?);
		}
		Ok(OutboundPage { items, next_cursor })
	}
	.await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(get,path="/capabilities/approvals",operation_id="core_approval_list",params(Page),responses((status=200,body=ApprovalPage)),security(("bearer_auth"=[])))]
async fn approval_list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	QueryInput(page): QueryInput<Page>,
) -> Result<Json<ApprovalPage>> {
	let mut access = access(&f, actor).await?;
	let result=async {
        let rows:Vec<super::records::Record>=sqlx::query_as(&sessions::select("core_records")
            .and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1"))).and_where(Expr::col(Alias::new("kind")).is_in(["outbound","grant"]))
            .and_where(Expr::cust("owner = $2 OR data->>'approver' = $2"))
            .and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$3"))).order_by(Alias::new("id"),Order::Asc).limit(51).to_string(PostgresQueryBuilder))
            .bind(&access.identity.tenant).bind(&access.identity.subject).bind(page.cursor.unwrap_or(Uuid::nil())).fetch_all(&mut **access.tx).await?;
        let next_cursor=(rows.len()==51).then(||rows[49].id);
        let mut items=vec![];
        for row in rows.into_iter().take(50) {
            if !super::approvals::visible(&access, &row)? { continue; }
            let run:Uuid=serde_json::from_value(row.data["run_id"].clone())?;
            // Designated approvers see only the scoped card, never Run output.
            items.push(json!({"id":row.id,"kind":row.kind,"run_id":run,"area_id":row.area_id,"state":if row.state=="pending"&&row.expires_at.is_some_and(|t|t<chrono::Utc::now()){"expired"}else{&row.state},"revision":row.revision,"requester":row.owner,"approver":row.data["approver"],"targets":row.data["targets"],"action":"network.get","expires_at":row.expires_at,"grant_id":row.data["grant_id"]}));
        }
        Ok(json!({"items":items,"next_cursor":next_cursor}))
    }.await;
	typed(access.finish(result).await?)
}
#[utoipa::path(post,path="/capabilities/approvals/{id}/decide",operation_id="core_approval_decide",params(("id"=Uuid,Path)),request_body=super::approvals::ApprovalDecision,responses((status=200,body=ApprovalDecided)),security(("bearer_auth"=[])))]
async fn approval_decide(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::approvals::ApprovalDecision>,
) -> Result<Json<ApprovalDecided>> {
	let mut access = access(&f, actor).await?;
	let result = super::approvals::decide(&f.store, &mut access, id, input).await;
	let value = access.finish(result).await?;
	f.notify.notify_waiters();
	typed(value)
}
#[utoipa::path(post,path="/capabilities/approvals/{id}/revoke",operation_id="core_approval_revoke",params(("id"=Uuid,Path)),request_body=super::approvals::Revoke,responses((status=200,body=Revoked)),security(("bearer_auth"=[])))]
async fn approval_revoke(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::approvals::Revoke>,
) -> Result<Json<Revoked>> {
	let mut access = access(&f, actor).await?;
	let result = super::approvals::revoke(&mut access, id, "outbound", input).await;
	typed(access.finish(result).await?)
}
#[utoipa::path(post,path="/capabilities/grants/{id}/revoke",operation_id="core_grant_revoke",params(("id"=Uuid,Path)),request_body=super::approvals::Revoke,responses((status=200,body=Revoked)),security(("bearer_auth"=[])))]
async fn grant_revoke(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::approvals::Revoke>,
) -> Result<Json<Revoked>> {
	let mut access = access(&f, actor).await?;
	let result = super::approvals::revoke(&mut access, id, "grant", input).await;
	typed(access.finish(result).await?)
}

#[utoipa::path(post,path="/runs/{id}/python",operation_id="core_python",params(("id"=Uuid,Path)),request_body=super::python::Python,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn python(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::python::Python>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "code_interpreter", json!(input)).await
}
#[utoipa::path(post,path="/runs/{id}/python/install",operation_id="core_python_install",params(("id"=Uuid,Path)),request_body=super::packages::Install,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn python_install(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::packages::Install>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "python_install", json!(input)).await
}
#[utoipa::path(post,path="/runs/{id}/python/poll",operation_id="core_python_poll",params(("id"=Uuid,Path)),request_body=OperationInput,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn python_poll(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "python_poll", json!(input)).await
}
#[utoipa::path(post,path="/runs/{id}/python/cancel",operation_id="core_python_cancel",params(("id"=Uuid,Path)),request_body=OperationInput,responses((status=200,body=Envelope)),security(("bearer_auth"=[])))]
async fn python_cancel(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> Result<Json<Envelope>> {
	dispatch(f, actor, id, "python_cancel", json!(input)).await
}

#[utoipa::path(post,path="/references/uploads",operation_id="reference_upload",request_body=super::references::Upload,responses((status=200,body=super::references::Reference)),security(("bearer_auth"=[])))]
async fn reference_upload(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Json(input): Json<super::references::Upload>,
) -> Result<Json<super::references::Reference>> {
	let mut access = access(&f, actor).await?;
	let result = super::references::start(&f.store, &mut access, input).await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(post,path="/references/{id}/chunks",operation_id="reference_chunk",params(("id"=Uuid,Path)),request_body=super::references::Chunk,responses((status=200,body=super::references::Reference)),security(("bearer_auth"=[])))]
async fn reference_chunk(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::references::Chunk>,
) -> Result<Json<super::references::Reference>> {
	let mut access = access(&f, actor).await?;
	let result = super::references::chunk(&f.store, &mut access, id, input).await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(post,path="/references/{id}/commit",operation_id="reference_commit",params(("id"=Uuid,Path)),responses((status=200,body=super::references::Reference)),security(("bearer_auth"=[])))]
async fn reference_commit(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<super::references::Reference>> {
	let mut access = access(&f, actor).await?;
	let result = super::references::commit(&f.store, &mut access, id).await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(get,path="/references/{id}",operation_id="reference_inspect",params(("id"=Uuid,Path)),responses((status=200,body=super::references::Reference)),security(("bearer_auth"=[])))]
async fn reference_inspect(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<super::references::Reference>> {
	let mut access = access(&f, actor).await?;
	let result = super::references::inspect(&mut access, id).await;
	access.finish(result).await.map(Json)
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct ExpectedRevision {
	expected_revision: i64,
}
#[utoipa::path(post,path="/references/{id}/revoke",operation_id="reference_revoke",params(("id"=Uuid,Path)),request_body=ExpectedRevision,responses((status=200,body=ReferenceRevoked)),security(("bearer_auth"=[])))]
async fn reference_revoke(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<ExpectedRevision>,
) -> Result<Json<ReferenceRevoked>> {
	let mut access = access(&f, actor).await?;
	let result = super::references::revoke(&mut access, id, input.expected_revision).await;
	access.finish(result).await?;
	typed(json!({"reference_id":id,"status":"revoked"}))
}
#[derive(Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
struct Offset {
	offset: Option<u64>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
struct DownloadChunk {
	file: FileEntry,
	offset: u64,
	data: String,
	next_offset: Option<u64>,
}
async fn download(
	store: &crate::store::Store,
	access: &mut Access,
	file: FileEntry,
	offset: u64,
) -> Result<DownloadChunk> {
	use base64::Engine;
	let bytes = store.capabilities.read_chunk(access, &file, offset).await?;
	let end = offset + bytes.len() as u64;
	Ok(DownloadChunk {
		next_offset: (end < file.size).then_some(end),
		file,
		offset,
		data: base64::engine::general_purpose::STANDARD.encode(bytes),
	})
}
#[utoipa::path(get,path="/references/{id}/download",operation_id="reference_download",params(("id"=Uuid,Path),Offset),responses((status=200,body=DownloadChunk)),security(("bearer_auth"=[])))]
async fn reference_download(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	QueryInput(input): QueryInput<Offset>,
) -> Result<Json<DownloadChunk>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let reference = super::references::get(&mut access, id, "reference.read").await?;
		let file = serde_json::from_value(reference.data["original"].clone())
			.map_err(|_| Error::Conflict("REFERENCE_NOT_READY".into()))?;
		download(&f.store, &mut access, file, input.offset.unwrap_or(0)).await
	}
	.await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(get,path="/working-areas/{id}/files/{file}/download",operation_id="core_file_download",params(("id"=Uuid,Path),("file"=Uuid,Path),Offset),responses((status=200,body=DownloadChunk)),security(("bearer_auth"=[])))]
async fn file_download(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, file)): Path<(Uuid, Uuid)>,
	QueryInput(input): QueryInput<Offset>,
) -> Result<Json<DownloadChunk>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let area = sessions::load(&mut access, id).await?;
		service::available(&area)?;
		let entry = if let Some(entry) = service::files(&area)?
			.into_iter()
			.find(|entry| entry.file_id == file)
		{
			entry
		} else {
			let meta: Option<(String, i64, String)> = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.columns(["digest", "size", "kind"].map(Alias::new))
					.from(Alias::new("core_objects"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$2")))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$3")))
					.and_where(Expr::col(Alias::new("kind")).is_in([
						"display",
						"output",
						"network_output",
					]))
					.to_string(PostgresQueryBuilder),
			)
			.bind(file)
			.bind(id)
			.bind(&access.identity.tenant)
			.fetch_optional(&mut **access.tx)
			.await?;
			let (digest, size, kind) =
				meta.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
			FileEntry {
				file_id: file,
				path: file.to_string(),
				digest,
				size: size as u64,
				media_type: if kind == "display" {
					"image/png"
				} else if kind == "network_output" {
					"application/octet-stream"
				} else {
					"text/plain; charset=utf-8"
				}
				.into(),
				scope: FileScope::Working,
				provenance: json!({"kind":kind,"area_id":id}),
			}
		};
		download(&f.store, &mut access, entry, input.offset.unwrap_or(0)).await
	}
	.await;
	access.finish(result).await.map(Json)
}

#[utoipa::path(get,path="/working-files",operation_id="core_file_management",params(Page),responses((status=200,body=super::cleanup::ManagementPage)),security(("bearer_auth"=[])))]
async fn managed_areas(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	QueryInput(input): QueryInput<Page>,
) -> Result<Json<super::cleanup::ManagementPage>> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::inventory(&mut access, input.cursor).await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(post,path="/working-areas/{id}/cleanup",operation_id="core_cleanup",params(("id"=Uuid,Path)),request_body=super::cleanup::Cleanup,responses((status=200,body=super::cleanup::CleanupResult)),security(("bearer_auth"=[])))]
async fn cleanup(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::cleanup::Cleanup>,
) -> Result<Json<super::cleanup::CleanupResult>> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::prepare(&f.store, &mut access, id, input).await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(get,path="/file-cleanups/{id}",operation_id="core_cleanup_status",params(("id"=Uuid,Path)),responses((status=200,body=super::cleanup::CleanupResult)),security(("bearer_auth"=[])))]
async fn cleanup_status(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<super::cleanup::CleanupResult>> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::status(&mut access, id).await;
	access.finish(result).await.map(Json)
}
#[utoipa::path(post,path="/file-cleanups/{id}/reconcile",operation_id="core_cleanup_reconcile",params(("id"=Uuid,Path)),responses((status=200,body=super::cleanup::CleanupResult)),security(("bearer_auth"=[])))]
async fn cleanup_reconcile(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<super::cleanup::CleanupResult>> {
	let mut access = access(&f, actor.clone()).await?;
	let result = async {
		super::cleanup::status(&mut access, id).await?;
		super::records::get(&mut access, id, "cleanup").await
	}
	.await;
	let record = access.finish(result).await?;
	// Reconcile a previously authorized, committed deletion intent. Release the
	// read transaction first; the job takes its own generation/ownership locks.
	super::cleanup::erase_job(&f.store, record).await?;
	cleanup_status(State(f), Extension(actor), Path(id)).await
}
#[utoipa::path(post,path="/working-areas/{id}/deletion-confirmation",operation_id="core_deletion_confirmation",params(("id"=Uuid,Path)),request_body=ExpectedRevision,responses((status=200,body=DeletionConfirmation)),security(("bearer_auth"=[])))]
async fn deletion_confirmation(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<ExpectedRevision>,
) -> Result<Json<DeletionConfirmation>> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::confirmation(&mut access, id, input.expected_revision).await;
	typed(access.finish(result).await?)
}
#[utoipa::path(post,path="/working-areas/{id}/restore",operation_id="core_restore",params(("id"=Uuid,Path)),request_body=super::cleanup::Restore,responses((status=200,body=Area)),security(("bearer_auth"=[])))]
async fn restore(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<super::cleanup::Restore>,
) -> Result<Json<Area>> {
	let mut access = access(&f, actor).await?;
	let result = super::cleanup::restore(&f.store, &mut access, id, input).await;
	access.finish(result).await.map(Json)
}

#[utoipa::path(get,path="/file-transfers/{id}",operation_id="file_transfer_status",params(("id"=Uuid,Path)),responses((status=200,body=TransferStatus)),security(("bearer_auth"=[])))]
async fn transfer_status(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<TransferStatus>> {
	let mut access = access(&f, actor).await?;
	let result = super::transfer::view(&mut access, id).await;
	typed(access.finish(result).await?)
}
#[utoipa::path(get,path="/working-areas/{id}/transfers",operation_id="file_transfer_history",params(("id"=Uuid,Path),Page),responses((status=200,body=TransferPage)),security(("bearer_auth"=[])))]
async fn transfer_history(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	QueryInput(page): QueryInput<Page>,
) -> Result<Json<TransferPage>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		super::cleanup::load(&mut access, id, "file.read").await?;
		let records: Vec<super::records::Record> = sqlx::query_as(
			&sessions::select("core_records")
				.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$3")))
				.and_where(Expr::col(Alias::new("kind")).is_in(["transfer_out", "collaboration"]))
				.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$4")))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(51)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(&access.identity.tenant)
		.bind(&access.identity.subject)
		.bind(page.cursor.unwrap_or(Uuid::nil()))
		.fetch_all(&mut **access.tx)
		.await?;
		let next_cursor = (records.len() == 51).then(|| records[49].id);
		let mut items = vec![];
		for record in records.into_iter().take(50) {
			items.push(if record.kind == "transfer_out" {
				super::transfer::view(&mut access, record.id).await?
			} else {
				record.data
			});
		}
		Ok(json!({"items":items,"next_cursor":next_cursor}))
	}
	.await;
	typed(access.finish(result).await?)
}
#[utoipa::path(post,path="/file-transfers/{id}/reconcile",operation_id="file_transfer_reconcile",params(("id"=Uuid,Path)),responses((status=200,body=TransferStatus)),security(("bearer_auth"=[])))]
async fn transfer_reconcile(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<TransferStatus>> {
	let mut access = access(&f, actor.clone()).await?;
	let result = super::transfer::view(&mut access, id).await;
	access.finish(result).await?;
	super::transfer::reconcile(&f, id).await?;
	transfer_status(State(f), Extension(actor), Path(id)).await
}
#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
struct RecipientQuery {
	node_id: String,
	cursor: Option<Uuid>,
}
#[utoipa::path(get,path="/file-recipients",operation_id="file_recipient_list",params(RecipientQuery),responses((status=200,body=RecipientPage)),security(("bearer_auth"=[])))]
async fn transfer_recipients(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	QueryInput(input): QueryInput<RecipientQuery>,
) -> Result<Json<RecipientPage>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		if input.node_id == f.config.node_id {
			return super::transfer::recipient_list(&f, &mut access, input.cursor).await;
		}
		access
			.require(
				&access.resource("node", &input.node_id, json!({})),
				"file.transfer",
			)
			.await?;
		crate::authorization::peer::authority_request(
			&f,
			&input.node_id,
			"/scoped/files/recipients",
			&json!(super::transfer::Requester {
				tenant: access.identity.tenant.clone(),
				subject: access.identity.subject.clone(),
				cursor: input.cursor
			}),
		)
		.await
	}
	.await;
	typed(access.finish(result).await?)
}

#[utoipa::path(get,path="/runs/{id}/core-operations",operation_id="core_operation_history",params(("id"=Uuid,Path),Page),responses((status=200,body=OperationPage)),security(("bearer_auth"=[])))]
async fn operation_history(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	QueryInput(page): QueryInput<Page>,
) -> Result<Json<OperationPage>> {
	let (mut access, run) = run_access(&f, actor, id).await?;
	let result=async {
        let area=sessions::for_run(&mut access,&run).await?;
        let rows:Vec<(Uuid,String,String)>=sqlx::query_as(&sea_orm::sea_query::Query::select().columns(["id","kind","state"].map(Alias::new)).from(Alias::new("core_operations"))
            .and_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
            .and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$2")))
            .and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$3"))).order_by(Alias::new("id"),Order::Asc).limit(33).to_string(PostgresQueryBuilder))
            .bind(run.id).bind(area.id).bind(page.cursor.unwrap_or(Uuid::nil())).fetch_all(&mut **access.tx).await?;
        let cursor=if rows.len()==33 {Some(rows[31].0)}else{None};
        Ok(json!({"items":rows.into_iter().take(32).map(|(id,kind,status)|json!({"operation_id":id,"kind":kind,"status":status})).collect::<Vec<_>>(),"next_cursor":cursor}))
    }.await;
	typed(access.finish(result).await?)
}
#[derive(serde::Serialize, utoipa::ToSchema)]
struct ReferencePage {
	items: Vec<super::references::Reference>,
	next_cursor: Option<Uuid>,
}
#[utoipa::path(get,path="/references",operation_id="reference_list",params(Page),responses((status=200,body=ReferencePage)),security(("bearer_auth"=[])))]
async fn reference_list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	QueryInput(page): QueryInput<Page>,
) -> Result<Json<ReferencePage>> {
	let mut access = access(&f, actor).await?;
	let result = async {
		let ids: Vec<Uuid> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("kind")).eq("reference"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("state")).is_not_in(["revoked", "expired"]))
				.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$3")))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(51)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.bind(&access.identity.subject)
		.bind(page.cursor.unwrap_or(Uuid::nil()))
		.fetch_all(&mut **access.tx)
		.await?;
		let next_cursor = if ids.len() == 51 { Some(ids[49]) } else { None };
		let mut items = vec![];
		for id in ids.into_iter().take(50) {
			match super::references::inspect(&mut access, id).await {
				Ok(v) => items.push(v),
				Err(Error::Forbidden | Error::NotFound(_)) => {}
				Err(e) => return Err(e),
			}
		}
		Ok(ReferencePage { items, next_cursor })
	}
	.await;
	access.finish(result).await.map(Json)
}

#[utoipa::path(post,path="/workspaces/{workspace}/threads/{thread}/delete",operation_id="core_thread_delete",params(("workspace"=Uuid,Path),("thread"=Uuid,Path)),request_body=super::thread_lifecycle::DeleteThread,responses((status=200,body=DeletedThread)),security(("bearer_auth"=[])))]
async fn thread_delete(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((workspace, thread)): Path<(Uuid, Uuid)>,
	Json(input): Json<super::thread_lifecycle::DeleteThread>,
) -> Result<Json<DeletedThread>> {
	let mut access = access(&f, actor).await?;
	let result =
		super::thread_lifecycle::delete(&f.store, &mut access, workspace, thread, input).await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	typed(result)
}
