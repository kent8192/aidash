//! Content-free control metadata remains available when dependent content is hidden.
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	domain::Run,
	federation::Federation,
};
use axum::{
	Extension, Json,
	extract::{Path, State},
};
use sea_orm::sea_query::{Alias, Asterisk, Expr, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

#[derive(Serialize, utoipa::ToSchema)]
pub struct RunManagement {
	pub id: Uuid,
	pub phase: String,
	pub control: String,
	pub semantic_reason: Option<crate::semantic::remote::Failure>,
}
impl From<Run> for RunManagement {
	fn from(run: Run) -> Self {
		Self {
			id: run.id,
			phase: run.phase,
			control: run.control,
			semantic_reason: run
				.pending
				.get("semantic_reason")
				.and_then(|v| serde_json::from_value(v.clone()).ok()),
		}
	}
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManagementAction {
	Pause,
	Cancel,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunManagementInput {
	pub action: ManagementAction,
}

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new().routes(routes!(get, control))
}

async fn authorized(access: &mut Access, id: Uuid, node: &str) -> Result<Run> {
	let run: Run = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&mut **access.tx)
	.await?
	.ok_or(Error::Forbidden)?;
	let workspace = if run.home_node == node {
		access.workspace(run.workspace_id).await?
	} else {
		let tenant: Option<String> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::cust("id=$1 AND source_node=$2 AND task_id=$3"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(&run.home_node)
		.bind(run.task_id)
		.fetch_optional(&mut **access.tx)
		.await?;
		if tenant.as_deref() != Some(&access.identity.tenant) {
			return Err(Error::Forbidden);
		}
		access.resource(
			"workspace",
			format!("{}/workspaces/{}", run.home_node, run.workspace_id),
			json!({}),
		)
	};
	access.require(&workspace, "workspace.read").await?;
	access
		.require(
			&access.resource("run", id, workspace.attributes),
			"run.control",
		)
		.await?;
	Ok(run)
}

#[utoipa::path(get,path="/runs/{id}/management",operation_id="run_management_get",params(("id"=Uuid,Path)),responses((status=200,body=RunManagement)),security(("bearer_auth"=[])))]
async fn get(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<RunManagement>> {
	let Actor::Subject(identity) = actor else {
		return Ok(Json(f.store.run(id).await?.into()));
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = authorized(&mut access, id, &f.config.node_id)
		.await
		.map(|r| Json(r.into()));
	access.finish(result).await
}

#[utoipa::path(post,path="/runs/{id}/management",operation_id="run_management_control",params(("id"=Uuid,Path)),request_body=RunManagementInput,responses((status=200,body=RunManagement)),security(("bearer_auth"=[])))]
async fn control(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<RunManagementInput>,
) -> Result<Json<RunManagement>> {
	let action = match input.action {
		ManagementAction::Pause => "pause",
		ManagementAction::Cancel => "cancel",
	};
	let Actor::Subject(identity) = actor else {
		let current = f.store.run(id).await?;
		let run = if current.control == "CANCELLED" {
			current
		} else {
			f.store.control(id, action).await?
		};
		f.notify.notify_waiters();
		return Ok(Json(run.into()));
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let run = authorized(&mut access, id, &f.config.node_id).await?;
		let run = if run.control == "CANCELLED" {
			run
		} else {
			f.store.control_in(&mut access.tx, id, action).await?
		};
		Ok(Json(run.into()))
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
}
