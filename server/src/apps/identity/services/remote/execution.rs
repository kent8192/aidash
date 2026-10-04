//! Source-side activation and command journal for subject-scoped remote work.
//! Receiver admissions never enter the legacy offer/workspace authority path.
#[path = "execution/commands.rs"]
pub(crate) mod commands;
use super::*;
use futures_util::{StreamExt, stream};
use reinhardt::query::{Alias, ColumnRef, Expr, LockType, PostgresQueryBuilder, Query};

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct HomeBinding {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub task_id: Uuid,
	pub task_revision: i64,
	pub initial_task: Value,
}
pub(crate) async fn binding(access: &mut Access, grant: Uuid) -> Result<Option<HomeBinding>> {
	Ok({
		let query_bind_1 = grant;
		sqlx::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("authorization_remote_execution"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("grant_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	})
}

pub(crate) async fn message(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
	input: RemoteExecutionMessageInput,
) -> Result<RemoteExecutionMessageReceipt> {
	crate::domain::nonempty(&input.content, "run message")?;
	let Actor::Subject(identity) = actor.clone() else {
		return Err(Error::Forbidden);
	};
	let grant = requester(&f, actor, task, id, true).await?;
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
	Ok(receipt)
}

pub(crate) async fn control(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
	input: RemoteExecutionControlInput,
) -> Result<RemoteExecutionActivation> {
	let Actor::Subject(identity) = actor.clone() else {
		return Err(Error::Forbidden);
	};
	let grant = requester(
		&f,
		actor,
		task,
		id,
		matches!(input.action, RemoteExecutionControl::Resume),
	)
	.await?;
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let task = managed_task(
			&mut access,
			task,
			matches!(input.action, RemoteExecutionControl::Resume),
		)
		.await?;
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
	if matches!(input.action, RemoteExecutionControl::Resume)
		&& !serde_json::from_value::<crate::semantic::remote::Binding>(grant.semantic.clone())?
			.disabled()
	{
		let (mut authority, description) = super::description_lease(&f, &grant.node_id, id).await?;
		let reset = crate::semantic::remote::journal::resume_in(
			&f.store,
			&mut authority.tx,
			id,
			admission,
			description.task.workspace_id,
			&identity.subject,
		)
		.await;
		authority.finish(reset).await?;
	}
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
			let current = managed_task(&mut access, task, false).await?;
			let resource = access.task_resource(&current).await?;
			access
				.require(
					&access.resource("run", admission, resource.attributes),
					"run.control",
				)
				.await?;
			// Revocation shares the lease row with every home command. Effects
			// already in progress finish before this write; later ones fail.
			{
				let query_bind_1 = id;
				let query_bind_2 = &identity.tenant;
				let query_bind_3 = &identity.subject;
				sqlx::query(
					&Query::update()
						.table(Alias::new("authorization_remote_grants"))
						.value(Alias::new("revoked"), true)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND tenant=? AND root_subject=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			if !matches!(
				current.status,
				crate::domain::TaskStatus::Cancelled
					| crate::domain::TaskStatus::Completed
					| crate::domain::TaskStatus::Failed
			) {
				let keys: Vec<String> = {
					let query_bind_1 = task;
					let query_bind_2 = admission;
					sqlx::query_scalar(
						&Query::select()
							.column(Alias::new("idempotency_key"))
							.from(Alias::new("remote_run_message_fences"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(task_id=? AND run_id=?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
								],
							))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_all(&mut **access.tx)
					.await?
				};
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
						crate::domain::TaskStatus::Cancelled,
						admission,
						crate::store::TerminalRunMessageInputs::Keys(&keys),
					)
					.await?;
				{
					let query_bind_1 = id;
					let query_bind_2 = cancelled.revision;
					sqlx::query(
						&Query::update()
							.table(Alias::new("authorization_remote_execution"))
							.value_expr(
								Alias::new("task_revision"),
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								),
							)
							.and_where(SimpleExpr::CustomWithExpr(
								"(grant_id=?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(PostgresQueryBuilder),
					)
					.execute(&mut **access.tx)
					.await?
				};
			}
			Ok(())
		}
		.await;
		access.finish(outcome).await?;
	}
	Ok(result)
}

pub(crate) async fn list(
	f: Federation,
	actor: Actor,
	task: Uuid,
) -> Result<Vec<RemoteExecutionStatus>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		managed_task(&mut access, task, false).await?;
		let grants: Vec<Grant> = {
			let query_bind_1 = task;
			let query_bind_2 = &identity.tenant;
			let query_bind_3 = &identity.subject;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("authorization_remote_grants"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id=? AND tenant=? AND root_subject=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.order_by(Alias::new("expires_at"), reinhardt::query::Order::Desc)
					.limit(100)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **access.tx)
			.await?
		};
		Ok(grants)
	}
	.await;
	let grants = access.finish(result).await?;
	let result: Vec<Result<RemoteExecutionStatus>> =
		stream::iter(grants.into_iter().map(|grant| {
			let f = &f;
			let identity = &identity;
			async move {
				let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
				let mut status: Result<Option<RemoteExecutionActivation>> =
					tokio::time::timeout_at(
						deadline,
						super::super::peer::authority_request(
							f,
							&grant.node_id,
							"/scoped/execution/status",
							&json!({"grant_id":grant.id}),
						),
					)
					.await
					.unwrap_or_else(|_| Err(Error::External("remote status unavailable".into())));
				let semantic_binding: crate::semantic::remote::Binding =
					serde_json::from_value(grant.semantic.clone())?;
				let mut reason = status
					.as_ref()
					.ok()
					.and_then(|r| r.as_ref())
					.and_then(|r| r.semantic_reason);
				if !semantic_binding.disabled() {
					if let Err(error) = &status {
						// An unavailable status must not start another traversal of
						// that peer. The whole per-grant RPC/check shares one deadline.
						reason = Some(super::semantic::failure(error));
					} else {
						let allowed = tokio::time::timeout_at(deadline, async {
							let mut reader = Access::begin(&f.store, identity).await?;
							let allowed = async {
								reader.remote_semantic_sources(grant.id).await?;
								reader.grant_output_visible(grant.id).await
							}
							.await;
							reader.finish(allowed).await
						})
						.await;
						match allowed {
							Ok(Ok(true)) => {}
							Ok(Err(Error::RemoteSemantic(failure))) => reason = Some(failure),
							Ok(_) => reason = Some(crate::semantic::remote::Failure::Authority),
							Err(_) => {
								reason = Some(crate::semantic::remote::Failure::Unavailable);
								status = Err(Error::External("remote status unavailable".into()));
							}
						}
					}
				}
				let semantic = crate::semantic::remote::status::load(
					&f.store,
					grant.id,
					&semantic_binding,
					reason,
				)
				.await?;
				Ok(RemoteExecutionStatus {
					semantic,
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
	Ok(result)
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
        let prior:Vec<Grant>={ let query_bind_1 = task.id; let query_bind_2 = node; let query_bind_3 = &identity.tenant; let query_bind_4 = &identity.subject; sqlx::query_as(&Query::select().column(Asterisk).from(Alias::new("authorization_remote_grants"))
            .and_where(SimpleExpr::CustomWithExpr("(task_id=? AND node_id=? AND tenant=? AND root_subject=? AND NOT revoked AND expires_at>CLOCK_TIMESTAMP())".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into()]))
            .order_by(Alias::new("expires_at"),reinhardt::query::Order::Desc).limit(100).to_string(PostgresQueryBuilder)).fetch_all(&mut **access.tx).await? };
        Ok(prior.into_iter().find(|g|g.inspection["agent"]["id"]==agent.id && g.inspection["agent"]["version"]==agent.version).map(|g|g.id))
    }.await;
	let prior = access.finish(result).await?;
	let id = prior.unwrap_or_else(Uuid::new_v4);
	if prior.is_none() {
		let _ = super::RemoteGrants { runtime: f.clone() }
			.prepare(
				Actor::Subject(identity.clone()),
				task,
				PrepareInput {
					id,
					node_id: node.into(),
					agent: agent.clone(),
					ttl_seconds: 3600,
					semantic: crate::semantic::remote::Request::Disabled {},
				},
			)
			.await?;
	}
	let _ = activate(f.clone(), Actor::Subject(identity.clone()), (task, id)).await?;
	Ok(crate::federation::Delegation {
		task_id: task,
		node_id: node.into(),
		agent_id: agent.id.clone(),
		agent_version: agent.version.clone(),
		delivered: true,
	})
}

async fn requester(
	f: &Federation,
	actor: Actor,
	task: Uuid,
	id: Uuid,
	read: bool,
) -> Result<Grant> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let task_resource = managed_task(&mut access, task, read).await?;
		let resource = access.task_resource(&task_resource).await?;
		access.require(&resource, "task.delegate").await?;
		let grant: Grant = {
			let query_bind_1 = id;
			let query_bind_2 = task;
			let query_bind_3 = &identity.tenant;
			let query_bind_4 = &identity.subject;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("authorization_remote_grants"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("task_id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("root_subject")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							)),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		}
		.ok_or(Error::Forbidden)?;
		Ok(grant)
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn activate(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
) -> Result<RemoteExecutionActivation> {
	let grant = requester(&f, actor, task, id, true).await?;
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
		{
			let query_bind_1 = id;
			let query_bind_2 = admission.id;
			let query_bind_3 = task;
			let query_bind_4 = description.task.revision;
			let query_bind_5 = json!(description.task);
			sqlx::query(&format!(
				"{} ON CONFLICT DO NOTHING",
				Query::insert()
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
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()]
							))
							.to_owned()
					)
					.to_owned()
					.to_string(PostgresQueryBuilder)
			))
			.execute(&mut **access.tx)
			.await?
		};
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
	Ok(activated)
}

/// The destination must verify this binding before creating its local Run.
pub(crate) async fn activation_binding(
	f: Federation,
	headers: HeaderMap,
	input: VerifyInput,
) -> Result<Uuid> {
	let (mut access, _) = super::description_lease(
		&f,
		crate::apps::identity::services::http_auth::peer_node(&headers)?,
		input.grant_id,
	)
	.await?;
	let result = async {
		Ok(binding(&mut access, input.grant_id)
			.await?
			.ok_or(Error::Forbidden)?
			.admission_id)
	}
	.await;
	access.finish(result).await
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use http::HeaderMap;

#[derive(Clone)]
pub struct RemoteExecutionManagement {
	pub(crate) runtime: Federation,
}
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RemoteExecutionManagement {
	tracing::trace!(
		service = "RemoteExecutionManagement",
		"creating injectable service"
	);
	RemoteExecutionManagement { runtime }
}

use reinhardt::injectable;

pub use crate::apps::identity::serializers::remote_execution::{
	RemoteExecutionActivation, RemoteExecutionControl, RemoteExecutionControlInput,
	RemoteExecutionMessageInput, RemoteExecutionMessageReceipt, RemoteExecutionStatus,
};

use reinhardt::query::SimpleExpr;

/// Management uses only the task's control attributes. Returning its text or
/// journal still requires the ordinary dependency-aware read path.
async fn managed_task(access: &mut Access, id: Uuid, read: bool) -> Result<Task> {
	if read {
		return access.task_read(id).await;
	}
	let task: Task = {
		let query_bind_1 = id;
		aidash_server::database::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("tasks"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or(Error::Forbidden)?;
	let resource = access.task_resource(&task).await?;
	access.require(&resource, "task.delegate").await?;
	Ok(task)
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowUpInput {
	pub id: Uuid,
	pub title: String,
	pub description: String,
	pub requirements: crate::registry::Search,
}

pub(crate) async fn follow_up(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
	input: FollowUpInput,
) -> Result<Task> {
	crate::domain::nonempty(&input.title, "follow-up title")?;
	crate::domain::nonempty(&input.description, "follow-up description")?;
	let grant = requester(&f, actor.clone(), task, id, false).await?;
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let prior = managed_task(&mut access, task, false).await?;
		let workspace = access.workspace(prior.workspace_id).await?;
		access.require(&workspace, "task.create").await?;
		let bound = binding(&mut access, id).await?.ok_or(Error::Forbidden)?;
		access
			.require(
				&access.resource("run", bound.admission_id, workspace.attributes),
				"run.control",
			)
			.await?;
		let key = format!("remote-follow-up:{}:{}", grant.id, input.id);
		// A parent/dependency link would import the invalid producer context.
		// Keep only an audit association, and use independently supplied intent.
		let created = f
			.store
			.create_task_in(
				&mut access.tx,
				prior.workspace_id,
				&crate::domain::NewTask {
					title: input.title,
					description: input.description,
					requirements: serde_json::to_value(input.requirements)?,
					dependencies: vec![],
					parent_id: None,
				},
				&identity.subject,
				Some(&key),
			)
			.await?;
		Ok(created)
	}
	.await;
	access.finish(result).await
}

use serde::Deserialize;

use reinhardt::query::ColumnRef::Asterisk;

use serde::Serialize;

pub(crate) async fn provenance(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
) -> Result<Option<crate::semantic::remote::status::Provenance>> {
	requester(&f, actor.clone(), task, id, true).await?;
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		if !access.grant_output_visible(id).await? {
			return Err(Error::Forbidden);
		}
		let receipt: Option<Value> = {
			let query_bind_1 = id;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("receipt"))
					.from(Alias::new("semantic_remote_operations"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(grant_id=? AND receipt IS NOT NULL)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.order_by(Alias::new("created_at"), reinhardt::query::Order::Desc)
					.order_by(Alias::new("id"), reinhardt::query::Order::Desc)
					.limit(1)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		};
		crate::semantic::remote::status::provenance(&mut access, &f.config.node_id, receipt).await
	}
	.await;
	access.finish(result).await
}

pub use aidash_domain::federation::execution::admission::{
	RemoteExecutionControlState, RemoteExecutionPhase,
};
