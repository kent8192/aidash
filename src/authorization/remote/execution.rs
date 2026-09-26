//! Source-side activation and command journal for subject-scoped remote work.
//! Receiver admissions never enter the legacy offer/workspace authority path.
pub(crate) mod commands;
use super::*;
use futures_util::{StreamExt, stream};
use sea_orm::sea_query::{
	Alias, Asterisk, Expr, LockType, OnConflict, PostgresQueryBuilder, Query,
};

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct HomeBinding {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub task_id: Uuid,
	pub task_revision: i64,
	pub initial_task: Value,
}
pub(crate) async fn binding(access: &mut Access, grant: Uuid) -> Result<Option<HomeBinding>> {
	Ok(sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("authorization_remote_execution"))
			.and_where(Expr::col(Alias::new("grant_id")).eq(Expr::cust("$1")))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(grant)
	.fetch_optional(&mut **access.tx)
	.await?)
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct RemoteExecutionActivation {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub run_id: Uuid,
	pub phase: String,
	pub control: String,
	pub error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RemoteExecutionStatus {
	pub grant: Prepared,
	pub execution: Option<RemoteExecutionActivation>,
	pub unavailable: bool,
}

#[derive(Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteExecutionControl {
	Pause,
	Resume,
	Cancel,
}
impl RemoteExecutionControl {
	pub(crate) fn action(&self) -> &'static str {
		match self {
			Self::Pause => "pause",
			Self::Resume => "resume",
			Self::Cancel => "cancel",
		}
	}
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutionControlInput {
	pub action: RemoteExecutionControl,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutionMessageInput {
	pub id: Uuid,
	pub content: String,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct RemoteExecutionMessageReceipt {
	pub id: Uuid,
	pub run_id: Uuid,
	pub accepted: bool,
}

#[utoipa::path(post,path="/tasks/{task}/remote-grants/{id}/messages",operation_id="remote_execution_message",params(("task"=Uuid,Path),("id"=Uuid,Path)),request_body=RemoteExecutionMessageInput,responses((status=200,body=RemoteExecutionMessageReceipt)),security(("bearer_auth"=[])))]
pub(crate) async fn message(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((task, id)): Path<(Uuid, Uuid)>,
	Json(input): Json<RemoteExecutionMessageInput>,
) -> Result<Json<RemoteExecutionMessageReceipt>> {
	crate::domain::nonempty(&input.content, "run message")?;
	let Actor::Subject(identity) = actor.clone() else {
		return Err(Error::Forbidden);
	};
	let grant = requester(&f, actor, task, id).await?;
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let task = access.task_read(task).await?;
		let resource = access.workspace(task.workspace_id).await?;
		access.require(&resource, "message.create").await?;
		let bound = binding(&mut access, id).await?.ok_or(Error::Forbidden)?;
		access
			.require(
				&access.resource("run", bound.admission_id, resource.attributes),
				"run.message",
			)
			.await?;
		Ok(bound.admission_id)
	}
	.await;
	let admission = access.finish(result).await?;
	let receipt: RemoteExecutionMessageReceipt = super::super::peer::authority_request(
		&f,
		&grant.node_id,
		&format!("/scoped/execution/admissions/{admission}/messages"),
		&json!({"grant_id":id,"message":input}),
	)
	.await?;
	if receipt.id != input.id || receipt.run_id != admission || !receipt.accepted {
		return Err(Error::Forbidden);
	}
	Ok(Json(receipt))
}

#[utoipa::path(post,path="/tasks/{task}/remote-grants/{id}/control",operation_id="remote_execution_control",params(("task"=Uuid,Path),("id"=Uuid,Path)),request_body=RemoteExecutionControlInput,responses((status=200,body=RemoteExecutionActivation)),security(("bearer_auth"=[])))]
pub(crate) async fn control(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((task, id)): Path<(Uuid, Uuid)>,
	Json(input): Json<RemoteExecutionControlInput>,
) -> Result<Json<RemoteExecutionActivation>> {
	let Actor::Subject(identity) = actor.clone() else {
		return Err(Error::Forbidden);
	};
	let grant = requester(&f, actor, task, id).await?;
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let task = access.task_read(task).await?;
		let resource = access.task_resource(&task).await?;
		let bound = binding(&mut access, id).await?.ok_or(Error::Forbidden)?;
		access
			.require(
				&access.resource("run", bound.admission_id, resource.attributes),
				"run.control",
			)
			.await?;
		Ok(bound.admission_id)
	}
	.await;
	let admission = access.finish(result).await?;
	let result = super::super::peer::authority_request(
		&f,
		&grant.node_id,
		&format!("/scoped/execution/admissions/{admission}/control"),
		&json!({"grant_id":id,"action":input.action}),
	)
	.await?;
	if matches!(input.action, RemoteExecutionControl::Cancel) {
		// Stop the receiver before closing its home ledger. No source locks
		// span the peer call: a worker may be publishing under its run fence.
		let mut access = Access::begin(&f.store, &identity).await?;
		let outcome = async {
			let current = access.task_read(task).await?;
			let resource = access.task_resource(&current).await?;
			access
				.require(
					&access.resource("run", admission, resource.attributes),
					"run.control",
				)
				.await?;
			// Revocation shares the lease row with every home command. Effects
			// already in progress finish before this write; later ones fail.
			sqlx::query(
				&Query::update()
					.table(Alias::new("authorization_remote_grants"))
					.value(Alias::new("revoked"), true)
					.and_where(Expr::cust("id=$1 AND tenant=$2 AND root_subject=$3"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(id)
			.bind(&identity.tenant)
			.bind(&identity.subject)
			.execute(&mut **access.tx)
			.await?;
			if current.status != "CANCELLED" {
				let keys: Vec<String> = sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("idempotency_key"))
						.from(Alias::new("remote_run_message_fences"))
						.and_where(Expr::cust("task_id=$1 AND run_id=$2"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(task)
				.bind(admission)
				.fetch_all(&mut **access.tx)
				.await?;
				let owner = qualified_agent(
					&grant.node_id,
					&grant.prepared()?.agent.id,
					&grant.prepared()?.agent.version,
				);
				let cancelled = f
					.store
					.transition_remote_terminal_in(
						&mut access.tx,
						task,
						current.revision,
						&owner,
						"CANCELLED",
						admission,
						crate::store::TerminalRunMessageInputs::Keys(&keys),
					)
					.await?;
				sqlx::query(
					&Query::update()
						.table(Alias::new("authorization_remote_execution"))
						.value(Alias::new("task_revision"), Expr::cust("$2"))
						.and_where(Expr::cust("grant_id=$1"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.bind(cancelled.revision)
				.execute(&mut **access.tx)
				.await?;
			}
			Ok(())
		}
		.await;
		access.finish(outcome).await?;
	}
	Ok(Json(result))
}

#[utoipa::path(get,path="/tasks/{task}/remote-executions",operation_id="remote_execution_list",params(("task"=Uuid,Path)),responses((status=200,body=[RemoteExecutionStatus])),security(("bearer_auth"=[])))]
pub(crate) async fn list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(task): Path<Uuid>,
) -> Result<Json<Vec<RemoteExecutionStatus>>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		access.task_read(task).await?;
		let grants: Vec<Grant> = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("authorization_remote_grants"))
				.and_where(Expr::cust("task_id=$1 AND tenant=$2 AND root_subject=$3"))
				.order_by(Alias::new("expires_at"), sea_orm::sea_query::Order::Desc)
				.limit(100)
				.to_string(PostgresQueryBuilder),
		)
		.bind(task)
		.bind(&identity.tenant)
		.bind(&identity.subject)
		.fetch_all(&mut **access.tx)
		.await?;
		Ok(grants)
	}
	.await;
	let grants = access.finish(result).await?;
	let result: Vec<Result<RemoteExecutionStatus>> =
		stream::iter(grants.into_iter().map(|grant| {
			let f = &f;
			async move {
				let status: Result<Option<RemoteExecutionActivation>> = tokio::time::timeout(
					std::time::Duration::from_secs(2),
					super::super::peer::authority_request(
						f,
						&grant.node_id,
						"/scoped/execution/status",
						&json!({"grant_id":grant.id}),
					),
				)
				.await
				.unwrap_or_else(|_| Err(Error::External("remote status unavailable".into())));
				Ok(RemoteExecutionStatus {
					grant: grant.prepared()?,
					unavailable: status.is_err(),
					execution: status.ok().flatten(),
				})
			}
		}))
		.buffered(4)
		.collect()
		.await;
	let result = result.into_iter().collect::<Result<Vec<_>>>()?;
	Ok(Json(result))
}

pub(crate) async fn delegate(
	f: &Federation,
	identity: &super::super::identity::SubjectIdentity,
	task: Uuid,
	node: &str,
	agent: &EntityRef,
) -> Result<crate::federation::Delegation> {
	let mut access = Access::begin(&f.store, identity).await?;
	let result=async {
        let task=access.task_read(task).await?;
        let resource=access.task_resource(&task).await?;
        access.require(&resource,"task.delegate").await?;
        let prior:Vec<Grant>=sqlx::query_as(&Query::select().column(Asterisk).from(Alias::new("authorization_remote_grants"))
            .and_where(Expr::cust("task_id=$1 AND node_id=$2 AND tenant=$3 AND root_subject=$4 AND NOT revoked AND expires_at>CLOCK_TIMESTAMP()"))
            .order_by(Alias::new("expires_at"),sea_orm::sea_query::Order::Desc).limit(100).to_string(PostgresQueryBuilder))
            .bind(task.id).bind(node).bind(&identity.tenant).bind(&identity.subject).fetch_all(&mut **access.tx).await?;
        Ok(prior.into_iter().find(|g|g.inspection["agent"]["id"]==agent.id && g.inspection["agent"]["version"]==agent.version).map(|g|g.id))
    }.await;
	let prior = access.finish(result).await?;
	let id = prior.unwrap_or_else(Uuid::new_v4);
	if prior.is_none() {
		let _ = super::prepare(
			State(f.clone()),
			Extension(Actor::Subject(identity.clone())),
			Path(task),
			Json(PrepareInput {
				id,
				node_id: node.into(),
				agent: agent.clone(),
				ttl_seconds: 3600,
			}),
		)
		.await?;
	}
	let _ = activate(
		State(f.clone()),
		Extension(Actor::Subject(identity.clone())),
		Path((task, id)),
	)
	.await?;
	Ok(crate::federation::Delegation {
		task_id: task,
		node_id: node.into(),
		agent_id: agent.id.clone(),
		agent_version: agent.version.clone(),
		delivered: true,
	})
}

async fn requester(f: &Federation, actor: Actor, task: Uuid, id: Uuid) -> Result<Grant> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let task_resource = access.task_read(task).await?;
		let resource = access.task_resource(&task_resource).await?;
		access.require(&resource, "task.delegate").await?;
		let grant: Grant = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("authorization_remote_grants"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("task_id")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$3")))
				.and_where(Expr::col(Alias::new("root_subject")).eq(Expr::cust("$4")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(task)
		.bind(&identity.tenant)
		.bind(&identity.subject)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or(Error::Forbidden)?;
		Ok(grant)
	}
	.await;
	access.finish(result).await
}

#[utoipa::path(post,path="/tasks/{task}/remote-grants/{id}/activate",operation_id="remote_execution_activate",params(("task"=Uuid,Path),("id"=Uuid,Path)),responses((status=200,body=RemoteExecutionActivation)),security(("bearer_auth"=[])))]
pub(crate) async fn activate(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((task, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<RemoteExecutionActivation>> {
	let grant = requester(&f, actor, task, id).await?;
	// Admission performs fresh source + receiver checks. No source policy row
	// locks are held across its callback to this node.
	let admission: super::super::peer::admission::Admission =
		super::super::peer::authority_request(
			&f,
			&grant.node_id,
			"/scoped/execution/admissions",
			&json!({"grant_id":id}),
		)
		.await?;
	if admission.grant_id != id
		|| admission.source_node != f.config.node_id
		|| admission.task_id != task
	{
		return Err(Error::Forbidden);
	}
	let (mut access, description) = super::description_lease(&f, &grant.node_id, id).await?;
	let result = async {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_remote_execution"))
				.columns(
					[
						"grant_id",
						"admission_id",
						"task_id",
						"task_revision",
						"initial_task",
					]
					.map(Alias::new),
				)
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("$3"),
					Expr::cust("$4"),
					Expr::cust("$5"),
				])
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(admission.id)
		.bind(task)
		.bind(description.task.revision)
		.bind(json!(description.task))
		.execute(&mut **access.tx)
		.await?;
		let current = binding(&mut access, id)
			.await?
			.ok_or_else(|| Error::Conflict("task already has another scoped execution".into()))?;
		if current.admission_id != admission.id
			|| current.task_id != task
			|| current.initial_task != json!(description.task)
		{
			return Err(Error::Conflict("remote execution identity changed".into()));
		}
		Ok(())
	}
	.await;
	access.finish(result).await?;
	let activated: RemoteExecutionActivation = super::super::peer::authority_request(
		&f,
		&grant.node_id,
		&format!("/scoped/execution/admissions/{}/activate", admission.id),
		&json!({"grant_id":id}),
	)
	.await?;
	if activated.run_id != admission.id
		|| activated.admission_id != admission.id
		|| activated.grant_id != id
	{
		return Err(Error::Forbidden);
	}
	Ok(Json(activated))
}

/// The destination must verify this binding before creating its local Run.
pub(crate) async fn activation_binding(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<VerifyInput>,
) -> Result<Json<Uuid>> {
	let (mut access, _) =
		super::description_lease(&f, crate::api::peer_node(&headers)?, input.grant_id).await?;
	let result = async {
		Ok(Json(
			binding(&mut access, input.grant_id)
				.await?
				.ok_or(Error::Forbidden)?
				.admission_id,
		))
	}
	.await;
	access.finish(result).await
}
