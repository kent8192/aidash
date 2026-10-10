//! Content-free control metadata remains available when dependent content is hidden.
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	domain::{RunControl, RunControlAction, RunInspection, RunPhase},
	federation::Federation,
};

use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::json;

use uuid::Uuid;

#[derive(Serialize, schemars::JsonSchema)]
pub struct RunManagement {
	pub id: Uuid,
	pub phase: RunPhase,
	pub control: RunControl,
	pub semantic_reason: Option<crate::semantic::remote::Failure>,
	/// Typed context-recovery pause reason; cleared on resume.
	pub context_reason: Option<aidash_domain::context::recovery::Failure>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub memory_cleanup:
		Option<crate::apps::knowledge::repositories::receiver_caches::CleanupStatus>,
}
impl From<RunInspection> for RunManagement {
	fn from(run: RunInspection) -> Self {
		Self {
			id: run.id,
			phase: run.phase,
			control: run.control,
			semantic_reason: run.recovery.as_ref().and_then(|r| r.semantic_reason),
			context_reason: run.recovery.as_ref().and_then(|r| r.context_reason),
			memory_cleanup: None,
		}
	}
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManagementAction {
	Pause,
	Cancel,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunManagementInput {
	pub action: ManagementAction,
}

async fn authorized(access: &mut Access, id: Uuid, node: &str) -> Result<RunInspection> {
	let raw: crate::domain::run_state::RawRun = {
		let query_bind_1 = id;
		aidash_server::database::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("runs"))
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
	let run = raw.inspect();
	let workspace = if run.home_node == node {
		access.workspace(run.workspace_id).await?
	} else {
		let tenant: Option<String> = {
			let query_bind_1 = id;
			let query_bind_2 = &run.home_node;
			let query_bind_3 = run.task_id;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("tenant"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND source_node=? AND task_id=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **access.tx)
			.await?
		};
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

pub(crate) async fn get(f: Federation, actor: Actor, id: Uuid) -> Result<RunManagement> {
	let Actor::Subject(identity) = actor else {
		let mut result: RunManagement = f.store.inspect_run(id).await?.into();
		let mut tx = crate::database::native::begin(&f.store.control_pool).await?;
		result.memory_cleanup =
			crate::apps::knowledge::repositories::receiver_caches::status(&mut tx, id).await?;
		tx.rollback().await?;
		return Ok(result);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let mut result: RunManagement =
			authorized(&mut access, id, &f.config.node_id).await?.into();
		result.memory_cleanup =
			crate::apps::knowledge::repositories::receiver_caches::status(&mut access.tx, id)
				.await?;
		Ok(result)
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn control(
	f: Federation,
	actor: Actor,
	id: Uuid,
	input: RunManagementInput,
) -> Result<RunManagement> {
	let action = match input.action {
		ManagementAction::Pause => RunControlAction::Pause,
		ManagementAction::Cancel => RunControlAction::Cancel,
	};
	let Actor::Subject(identity) = actor else {
		let current = f.store.inspect_run(id).await?;
		let run = if current.control == RunControl::Cancelled {
			current
		} else {
			f.store.control(id, action).await?
		};
		f.notify.notify_waiters();
		return Ok(run.into());
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let run = authorized(&mut access, id, &f.config.node_id).await?;
		let run = if run.control == RunControl::Cancelled {
			run
		} else {
			f.store.control_in(&mut access.tx, id, action).await?
		};
		Ok(run.into())
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::ColumnRef::Asterisk;

use reinhardt::query::SimpleExpr;
