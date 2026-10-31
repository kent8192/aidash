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
	/// The exact semantic cause and its authorized next actions while the Run
	/// can still be recovered; absent for a cancelled or finished Run.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub semantic_recovery: Option<aidash_domain::semantic::recovery::Recovery>,
	/// Typed context-recovery pause reason; cleared on resume.
	pub context_reason: Option<aidash_domain::context::recovery::Failure>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub memory_cleanup:
		Option<crate::apps::knowledge::repositories::receiver_caches::CleanupStatus>,
}
impl From<RunInspection> for RunManagement {
	fn from(run: RunInspection) -> Self {
		let recoverable = run.control != RunControl::Cancelled && !run.phase.is_terminal();
		Self {
			id: run.id,
			phase: run.phase,
			control: run.control,
			semantic_reason: run.recovery.as_ref().and_then(|r| r.semantic_reason),
			// A paused Run is excluded from retry scheduling, so its retry only
			// counts as scheduled while the Run is active.
			semantic_recovery: run.recovery.as_ref().filter(|_| recoverable).and_then(|r| {
				r.semantic_reason.map(|cause| {
					aidash_domain::semantic::recovery::Recovery::new(
						cause,
						r.retry.is_some() && run.control == RunControl::Active,
					)
				})
			}),
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

/// `GET` query. Only an administrator names the Tenant.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct ManagementQuery {
	pub tenant: Option<String>,
}

/// What `GET` returns: a Subject's diagnostics, or an administrator's metadata.
#[derive(Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ManagementView {
	Subject(RunManagement),
	Administrator(super::safety_stop::RunControlFacts),
}

/// What `POST` returns: the Run for a Subject, or the receipt for an administrator.
#[derive(Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ManagementResult {
	Subject(RunManagement),
	Administrator(aidash_application::safety_stop::Receipt),
}

/// The body of `POST`. The two contracts share the route; the actor decides
/// which applies, so an unmatched body is kept for a precise refusal.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ManagementRequest {
	Subject(RunManagementInput),
	Administrator(super::safety_stop::SafetyStopInput),
	#[schemars(skip)]
	Unmatched(serde_json::Value),
}

const ADMINISTRATOR_ONLY: [&str; 3] = ["tenant", "expected_control_revision", "reason"];

impl ManagementRequest {
	/// The Subject contract, unchanged; administrator-only fields are refused.
	pub fn for_subject(self) -> Result<RunManagementInput> {
		match self {
			Self::Subject(input) => Ok(input),
			Self::Administrator(_) => Err(administrator_only()),
			Self::Unmatched(value) => {
				if value.as_object().is_some_and(|fields| {
					ADMINISTRATOR_ONLY
						.iter()
						.any(|name| fields.contains_key(*name))
				}) {
					return Err(administrator_only());
				}
				match serde_json::from_value::<RunManagementInput>(value) {
					Ok(input) => Ok(input),
					Err(error) => Err(Error::Unprocessable(error.to_string())),
				}
			}
		}
	}

	/// The administrator contract: Tenant, action, expected control revision, reason.
	pub fn for_administrator(self) -> Result<super::safety_stop::SafetyStopInput> {
		match self {
			Self::Administrator(input) => Ok(input),
			Self::Subject(_) | Self::Unmatched(_) => Err(Error::Invalid(
				"a Deployment Administrator stop names the tenant, action, \
				 expected_control_revision and reason"
					.into(),
			)),
		}
	}
}

fn administrator_only() -> Error {
	Error::Invalid(
		"tenant, expected_control_revision and reason are for Deployment Administrators".into(),
	)
}

pub(crate) async fn authorized(access: &mut Access, id: Uuid, node: &str) -> Result<RunInspection> {
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
	let workspace = if run.is_local(node) {
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
	// A Deployment Administrator's stop has its own content-free route handlers
	// (`safety_stop`); it never reaches the Subject diagnostics below.
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let mut result: RunManagement = authorized(&mut access, id, f.config.deployment.as_str())
			.await?
			.into();
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
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let run = authorized(&mut access, id, f.config.deployment.as_str()).await?;
		let run = if run.control == RunControl::Cancelled {
			run
		} else {
			f.store.control_in(&mut access.tx, id, action).await?
		};
		Ok(run.into())
	}
	.await;
	let result: RunManagement = access.finish(result).await?;
	f.store.run_control_committed(result.id, result.control);
	f.notify.notify_waiters();
	Ok(result)
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::ColumnRef::Asterisk;

use reinhardt::query::SimpleExpr;

#[cfg(test)]
#[path = "management/tests.rs"]
mod tests;
