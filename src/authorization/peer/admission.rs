//! Receiver admission binds a source grant to one local identity and executor.
//! RemoteExecutionActivation consumes this record only after home durably binds its run ID.
//! Worker boundaries revalidate the same authority; legacy offers stay closed.
use super::execution::{InspectInput, inspect_in};
use crate::domain::Run;
use crate::registry::AgentConfig;
use crate::{
	Error, Result,
	authorization::{access::Access, remote::Description},
	federation::Federation,
	registry::EntityRef,
};
use axum::{
	Json,
	extract::{Path, State},
	http::HeaderMap,
};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{
	Alias, Asterisk, Expr, LockType, OnConflict, PostgresQueryBuilder, Query,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	grant_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteExecutionControlInput {
	grant_id: Uuid,
	action: crate::authorization::remote::execution::RemoteExecutionControl,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageInput {
	grant_id: Uuid,
	message: crate::authorization::remote::execution::RemoteExecutionMessageInput,
}

pub(crate) async fn message(
	State(f): State<Federation>,
	headers: HeaderMap,
	Path(id): Path<Uuid>,
	Json(input): Json<MessageInput>,
) -> Result<Json<crate::authorization::remote::execution::RemoteExecutionMessageReceipt>> {
	let source = crate::api::peer_node(&headers)?;
	let run = f.store.run(id).await?;
	if run.home_node != source || run_grant(&f.store, &run).await? != Some(input.grant_id) {
		return Err(Error::Forbidden);
	}
	let (mut access, _) = worker_lease(&f, &run).await?.ok_or(Error::Forbidden)?;
	let preflight = async {
		access
			.require(&access.resource("run", id, json!({})), "run.message")
			.await?;
		access
			.require(
				&access.resource(
					"workspace",
					format!("{source}/workspaces/{}", run.workspace_id),
					json!({}),
				),
				"message.create",
			)
			.await?;
		Ok(())
	}
	.await;
	access.finish(preflight).await?;
	let home = crate::federation::Home::new(f.clone(), run.clone());
	let key = format!("human:{id}:{}", input.message.id);
	let limit = f.run_message_limit(&run).await?;
	if !home
		.reserve_run_message(&key, &input.message.content)
		.await?
	{
		return Err(Error::Forbidden);
	}
	// Reacquire receiver authority after reserving at home. A revocation
	// between reservation and admission cannot create an executable input.
	let (mut access, _) = worker_lease(&f, &run).await?.ok_or(Error::Forbidden)?;
	let sender = access.identity.subject.clone();
	let result = f
		.store
		.accept_run_message_in(
			&mut access.tx,
			id,
			&sender,
			&input.message.content,
			&key,
			limit,
		)
		.await;
	if let Err(error) = access.finish(result).await
		&& f.store
			.run_input_sequence(id, &key, &input.message.content)
			.await
			.is_err()
	{
		let _ = home.release_run_messages(std::slice::from_ref(&key)).await;
		return Err(error);
	}
	home.commit_run_message(&key, &input.message.content)
		.await?;
	f.deliver_run_messages(&run).await?;
	f.notify.notify_waiters();
	Ok(Json(
		crate::authorization::remote::execution::RemoteExecutionMessageReceipt {
			id: input.message.id,
			run_id: id,
			accepted: true,
		},
	))
}

pub(crate) async fn control(
	State(f): State<Federation>,
	headers: HeaderMap,
	Path(id): Path<Uuid>,
	Json(input): Json<RemoteExecutionControlInput>,
) -> Result<Json<crate::authorization::remote::execution::RemoteExecutionActivation>> {
	let source = crate::api::peer_node(&headers)?;
	let run = f.store.run(id).await?;
	if run.home_node != source || run_grant(&f.store, &run).await? != Some(input.grant_id) {
		return Err(Error::Forbidden);
	}
	if matches!(
		input.action,
		crate::authorization::remote::execution::RemoteExecutionControl::Resume
	) {
		let (access, _) = worker_lease(&f, &run).await?.ok_or(Error::Forbidden)?;
		access.finish(Ok(())).await?;
	}
	let run = if matches!(
		input.action,
		crate::authorization::remote::execution::RemoteExecutionControl::Cancel
	) && run.control == "CANCELLED"
	{
		run
	} else {
		f.store.control(id, input.action.action()).await?
	};
	f.notify.notify_waiters();
	Ok(Json(
		crate::authorization::remote::execution::RemoteExecutionActivation {
			grant_id: input.grant_id,
			admission_id: id,
			run_id: id,
			phase: run.phase,
			control: run.control,
			error: run
				.error
				.map(|_| "Remote execution requires attention.".into()),
		},
	))
}
#[derive(sqlx::FromRow)]
struct Record {
	id: Uuid,
	source_node: String,
	grant_id: Uuid,
	task_id: Uuid,
	tenant: String,
	credential_id: Uuid,
	subject_chain: Vec<String>,
	description: Value,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Admission {
	pub(crate) id: Uuid,
	pub(crate) source_node: String,
	pub(crate) grant_id: Uuid,
	pub(crate) task_id: Uuid,
	pub(crate) expires_at: DateTime<Utc>,
}

pub(crate) async fn status(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Input>,
) -> Result<Json<Option<crate::authorization::remote::execution::RemoteExecutionActivation>>> {
	let source = crate::api::peer_node(&headers)?;
	let record: Option<Record> = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::cust("source_node=$1 AND grant_id=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(source)
	.bind(input.grant_id)
	.fetch_optional(&f.store.pool)
	.await?;
	let Some(record) = record else {
		return Ok(Json(None));
	};
	let run: Option<Run> = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::cust("id=$1 AND home_node=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(record.id)
	.bind(source)
	.fetch_optional(&f.store.pool)
	.await?;
	Ok(Json(Some(
		crate::authorization::remote::execution::RemoteExecutionActivation {
			grant_id: input.grant_id,
			admission_id: record.id,
			run_id: record.id,
			phase: run.as_ref().map_or("ADMITTED", |r| r.phase.as_str()).into(),
			control: run
				.as_ref()
				.map_or("INACTIVE", |r| r.control.as_str())
				.into(),
			error: run.and_then(|r| {
				r.error.map(|_| {
					"Remote execution requires attention. Review current authority and retry controls.".into()
				})
			}),
		},
	)))
}
impl Record {
	fn matches(&self, access: &Access, description: &Description) -> Result<bool> {
		Ok(self.source_node == description.source_node
			&& self.grant_id == description.grant_id
			&& self.task_id == description.task.id
			&& self.tenant == access.identity.tenant
			&& self.credential_id == access.identity.credential_id
			&& self.subject_chain == access.subjects
			&& self.description == serde_json::to_value(description)?)
	}
	fn view(&self, description: &Description) -> Admission {
		Admission {
			id: self.id,
			source_node: self.source_node.clone(),
			grant_id: self.grant_id,
			task_id: self.task_id,
			expires_at: description.expires_at,
		}
	}
}

async fn lease(f: &Federation, source: &str, grant: Uuid) -> Result<(Access, Description)> {
	// Do not hold receiver authority while calling home: home's verification
	// inspects this receiver too, and a queued policy writer must not deadlock
	// two nested shared leases. Recheck the receiver under a fresh lease after
	// the source reply and retain it until the local binding is committed.
	let description: Description = super::authority_request(
		f,
		source,
		"/scoped/execution/grants/describe",
		&json!({"grant_id":grant}),
	)
	.await?;
	if description.source_node != source
		|| description.target_node != f.config.node_id
		|| description.grant_id != grant
		|| description.task.status != "OPEN"
	{
		return Err(Error::Forbidden);
	}
	let mut access = super::access(
		f,
		source,
		&description.source_tenant,
		&description.source_subject,
	)
	.await?;
	let result = async {
		access.context["source_workspace_id"] = json!(description.task.workspace_id);
		access.context["source_task_id"] = json!(description.task.id);
		access.context["source_task_revision"] = json!(description.task.revision);
		let fresh = inspect_in(
			f,
			&mut access,
			source,
			&InspectInput {
				tenant: description.source_tenant.clone(),
				subject: description.source_subject.clone(),
				agent: EntityRef {
					id: description.inspection.agent.id.clone(),
					version: description.inspection.agent.version.clone(),
				},
				requirements: serde_json::from_value(description.task.requirements.clone())?,
			},
		)
		.await?;
		if fresh != description.inspection {
			return Err(Error::Forbidden);
		}
		let workspace = access.resource(
			"workspace",
			format!("{source}/workspaces/{}", description.task.workspace_id),
			json!({}),
		);
		access.require(&workspace, "workspace.read").await?;
		let task = access.resource(
			"task",
			format!("{source}/tasks/{}", description.task.id),
			json!({"created_by":description.task.created_by,"requirements":description.task.requirements}),
		);
		access.require(&task, "task.read").await?;
		access.require(&task, "task.execute").await?;
		let live: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"CAST($1 AS TIMESTAMPTZ) > CLOCK_TIMESTAMP()",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(description.expires_at)
		.fetch_one(&mut **access.tx)
		.await?;
		if !live {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return access.finish(Err(error)).await;
	}
	Ok((access, description))
}

fn require_workspace_agent(agent: &AgentConfig) -> Result<()> {
	// Local working areas require a local conversation; a foreign workspace
	// grant cannot manufacture that conversation or inherit its private files.
	if agent.core_capabilities.enabled() {
		return Err(Error::Invalid("remote execution requires a workspace Agent without local core working-area capabilities; transfer files into an explicitly admitted local thread".into()));
	}
	Ok(())
}

pub(crate) async fn admit(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Input>,
) -> Result<Json<Admission>> {
	let source = crate::api::peer_node(&headers)?;
	let (mut access, description) = lease(&f, source, input.grant_id).await?;
	let result = async {
        let agent: AgentConfig = serde_json::from_value(description.inspection.agent.config.clone())?;
        require_workspace_agent(&agent)?;
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED($1, 71003209))",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(format!("{source}:{}", description.task.id))
		.execute(&mut **access.tx)
		.await?;
		let legacy: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM runs r WHERE home_node = $1 AND task_id = $2 AND NOT EXISTS(SELECT 1 FROM authorization_remote_admissions a WHERE a.id = r.id AND a.source_node = r.home_node AND a.grant_id = $3))",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(source)
		.bind(description.task.id)
		.bind(input.grant_id)
		.fetch_one(&mut **access.tx)
		.await?;
		if legacy {
			return Err(Error::Conflict(
				"task already has an incompatible execution".into(),
			));
		}
		let proposed = Uuid::new_v4();
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new(
					"authorization_remote_admissions",
				))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("source_node"),
					sea_orm::sea_query::Alias::new("grant_id"),
					sea_orm::sea_query::Alias::new("task_id"),
					sea_orm::sea_query::Alias::new("tenant"),
					sea_orm::sea_query::Alias::new("credential_id"),
					sea_orm::sea_query::Alias::new("subject_chain"),
					sea_orm::sea_query::Alias::new("description"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
					sea_orm::sea_query::Expr::cust("$6"),
					sea_orm::sea_query::Expr::cust("$7"),
					sea_orm::sea_query::Expr::cust("$8"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::new()
						.do_nothing()
						.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(proposed)
		.bind(source)
		.bind(input.grant_id)
		.bind(description.task.id)
		.bind(&access.identity.tenant)
		.bind(access.identity.credential_id)
		.bind(&access.subjects)
		.bind(serde_json::to_value(&description)?)
		.execute(&mut **access.tx)
		.await?;
		let record: Record = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new(
					"authorization_remote_admissions",
				))
				.and_where(sea_orm::sea_query::Expr::cust(
					"source_node = $1 AND grant_id = $2",
				))
				.lock(sea_orm::sea_query::LockType::Share)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(source)
		.bind(input.grant_id)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or_else(|| Error::Conflict("source task already has a different admission".into()))?;
		if !record.matches(&access, &description)? {
			return Err(Error::Conflict(
				"admission already binds different authority".into(),
			));
		}
		let live: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"CAST($1 AS TIMESTAMPTZ) > CLOCK_TIMESTAMP()",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(description.expires_at)
		.fetch_one(&mut **access.tx)
		.await?;
		if !live {
			return Err(Error::Forbidden);
		}
		// Policy decisions are retained by Access. No unscoped workspace event
		// may disclose this source task to receiver tenants.
		Ok(Json(record.view(&description)))
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn verify(
	State(f): State<Federation>,
	headers: HeaderMap,
	Path(id): Path<Uuid>,
) -> Result<Json<bool>> {
	let source = crate::api::peer_node(&headers)?;
	let record: Record = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new(
				"authorization_remote_admissions",
			))
			.and_where(sea_orm::sea_query::Expr::cust(
				"id = $1 AND source_node = $2",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.bind(source)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or(Error::Forbidden)?;
	let (access, description) = lease(&f, source, record.grant_id).await?;
	let result = if record.matches(&access, &description)? {
		Ok(Json(true))
	} else {
		Err(Error::Forbidden)
	};
	access.finish(result).await
}

/// A scoped record is never treated as a missing legacy grant, even after expiry.
pub(crate) async fn run_grant(store: &crate::store::Store, run: &Run) -> Result<Option<Uuid>> {
	let record: Option<Record> = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_optional(&store.pool)
	.await?;
	let Some(record) = record else {
		return Ok(None);
	};
	let d: Description = serde_json::from_value(record.description)?;
	if record.source_node != run.home_node
		|| record.task_id != run.task_id
		|| d.task.workspace_id != run.workspace_id
		|| d.inspection.agent.id != run.agent_id
		|| d.inspection.agent.version != run.agent_version
	{
		return Err(Error::Forbidden);
	}
	Ok(Some(record.grant_id))
}

pub(crate) async fn worker_lease(
	f: &Federation,
	run: &Run,
) -> Result<Option<(Access, AgentConfig)>> {
	let Some(grant) = run_grant(&f.store, run).await? else {
		return Ok(None);
	};
	let (mut access, d) = lease(f, &run.home_node, grant).await?;
	let result = async {
		let record: Record = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(run.id)
		.fetch_one(&mut **access.tx)
		.await?;
		if !record.matches(&access, &d)? {
			return Err(Error::Forbidden);
		}
		let agent: AgentConfig = serde_json::from_value(d.inspection.agent.config)?;
		access.durable_audit = true;
		access.worker();
		Ok(agent)
	}
	.await;
	match result {
		Ok(agent) => Ok(Some((access, agent))),
		Err(e) => access.finish(Err(e)).await,
	}
}

pub(crate) async fn activate(
	State(f): State<Federation>,
	headers: HeaderMap,
	Path(id): Path<Uuid>,
	Json(input): Json<Input>,
) -> Result<Json<crate::authorization::remote::execution::RemoteExecutionActivation>> {
	let source = crate::api::peer_node(&headers)?;
	let bound: Uuid = super::authority_request(
		&f,
		source,
		"/scoped/execution/grants/activation",
		&json!({"grant_id":input.grant_id}),
	)
	.await?;
	if bound != id {
		return Err(Error::Forbidden);
	}
	let (mut access, d) = lease(&f, source, input.grant_id).await?;
	let result = async {
		let record: Record = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or(Error::Forbidden)?;
		if !record.matches(&access, &d)? {
			return Err(Error::Forbidden);
		}
		let agent: AgentConfig = serde_json::from_value(d.inspection.agent.config.clone())?;
		require_workspace_agent(&agent)?;
		sqlx::query(
			&Query::select()
				.expr(Expr::cust(
					"PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED($1, 71003209))",
				))
				.to_string(PostgresQueryBuilder),
		)
		.bind(format!("{source}:{}", d.task.id))
		.execute(&mut **access.tx)
		.await?;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("runs"))
				.columns(
					[
						"id",
						"task_id",
						"workspace_id",
						"home_node",
						"agent_id",
						"agent_version",
					]
					.map(Alias::new),
				)
				.values_panic((1..=6).map(|i| Expr::cust(format!("${i}"))))
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(d.task.id)
		.bind(d.task.workspace_id)
		.bind(source)
		.bind(&d.inspection.agent.id)
		.bind(&d.inspection.agent.version)
		.execute(&mut **access.tx)
		.await?;
		let run: Run = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("runs"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or_else(|| Error::Conflict("source task already has another execution".into()))?;
		if run.task_id != d.task.id
			|| run.workspace_id != d.task.workspace_id
			|| run.home_node != source
			|| run.agent_id != d.inspection.agent.id
			|| run.agent_version != d.inspection.agent.version
		{
			return Err(Error::Forbidden);
		}
		Ok(Json(
			crate::authorization::remote::execution::RemoteExecutionActivation {
				grant_id: input.grant_id,
				admission_id: id,
				run_id: id,
				phase: run.phase,
				control: run.control,
				error: run.error,
			},
		))
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
}
