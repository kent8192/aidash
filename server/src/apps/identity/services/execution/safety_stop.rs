//! A Deployment Administrator's Safety Stop of one Run (ADR 0101): HTTP-facing
//! inputs and the persistence adapter of the application use case.
//!
//! The route names the Tenant and the Run. The Run's Tenant is read once, under
//! the read-only `RunTenant` scope, only to compare it with the named one; every
//! other read and write happens in the named Tenant's scope.
use crate::{
	Error, Result, apps::execution::models::safety_stops::RunSafetyStop,
	authorization::remote::deployment_administrator, database::native::Transaction,
	federation::Federation, store::Store,
};
use aidash_application::{
	safety_stop::{Control, Ledger, Receipt, Stored, execute},
	tenancy::TenancyScope,
};
use aidash_domain::{
	ControlSource, RunControl, RunControlAction, RunPhase,
	safety_stop::{Action, Actor, Command, Outcome},
	tenancy::TenantName,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The only facts a Deployment Administrator reads of a Run.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RunControlFacts {
	pub id: Uuid,
	pub phase: RunPhase,
	pub control: RunControl,
	/// Changes only when control changes; send it back with a stop.
	pub control_revision: i64,
	pub control_source: ControlSource,
}

/// An administrator's command. The operation ID is the `Idempotency-Key` header.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SafetyStopInput {
	pub tenant: String,
	pub action: Action,
	pub expected_control_revision: i64,
	pub reason: String,
}

/// The Run's Tenant, when the Run exists and belongs to `tenant`. A missing Run
/// and another Tenant's Run are the same answer.
async fn target(f: &Federation, tenant: &str, run: Uuid) -> Result<TenancyScope> {
	let named = TenantName::new(tenant)?;
	match f.store.run_tenancy(run).await {
		Ok(scope) if scope.tenant_name() == Some(&named) => Ok(scope),
		Ok(_) | Err(Error::NotFound(_)) => Err(Error::NotFound("run".into())),
		Err(error) => Err(error),
	}
}

fn facts_query(run: Uuid, lock: bool) -> String {
	let mut query = Query::select();
	query
		.columns(["phase", "control", "control_revision", "control_source"].map(Alias::new))
		.from(Alias::new("runs"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run)));
	if lock {
		// `NoKeyUpdate`, like `Run::hold_worker_lease`: a worker's retained
		// authority transaction holds a key-share lock through its foreign keys,
		// and `FOR UPDATE` would wait behind it. The stop never changes the key.
		query.lock(LockType::NoKeyUpdate);
	}
	query.to_string(PostgresQueryBuilder)
}

type Row = (String, String, i64, String);

async fn read_row(tx: &mut Transaction, run: Uuid, lock: bool) -> Result<Option<Row>> {
	crate::database::native::query_as::<Row>(&facts_query(run, lock))
		.columns(&["phase", "control", "control_revision", "control_source"])
		.fetch_optional(&mut **tx)
		.await
}

/// The Run's allowlisted metadata; a deleted Tenant is refused.
pub(crate) async fn facts(
	f: &Federation,
	id: Uuid,
	tenant: Option<String>,
) -> Result<RunControlFacts> {
	let tenant = tenant.ok_or_else(|| {
		Error::Invalid("a Deployment Administrator names the Tenant with ?tenant=".into())
	})?;
	let scope = target(f, &tenant, id).await?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result = async {
		deployment_administrator::tenant(&mut *tx, &tenant).await?;
		let (phase, control, control_revision, control_source) = read_row(&mut tx, id, false)
			.await?
			.ok_or_else(|| Error::NotFound("run".into()))?;
		Ok(RunControlFacts {
			id,
			phase: phase.try_into()?,
			control: control.try_into()?,
			control_revision,
			control_source: control_source.try_into()?,
		})
	}
	.await;
	tx.rollback().await?;
	result
}

struct Stop<'a> {
	store: &'a Store,
	tx: &'a mut Transaction,
	tenant: &'a str,
}

#[async_trait]
impl Ledger for Stop<'_> {
	async fn tenant_available(&mut self) -> aidash_application::Result<()> {
		deployment_administrator::tenant(&mut *self.tx, self.tenant)
			.await
			.map_err(deployment_administrator::application_error)
	}

	async fn lock_run(&mut self, run: Uuid) -> aidash_application::Result<Option<Control>> {
		let row = read_row(self.tx, run, true).await?;
		row.map(|(phase, control, control_revision, _)| {
			Ok(Control {
				phase: phase.try_into()?,
				control: control.try_into()?,
				control_revision,
			})
		})
		.transpose()
	}

	async fn receipt(
		&mut self,
		caller_key: &str,
		operation: Uuid,
	) -> aidash_application::Result<Option<Stored>> {
		Ok(RunSafetyStop::receipt(&mut *self.tx, self.tenant, caller_key, operation).await?)
	}

	async fn apply(
		&mut self,
		run: Uuid,
		action: Action,
		_: Actor,
	) -> aidash_application::Result<()> {
		let action = match action {
			Action::Pause => RunControlAction::Pause,
			Action::Cancel => RunControlAction::Cancel,
		};
		self.store
			.control_in_as(self.tx, run, action, ControlSource::DeploymentAdministrator)
			.await?;
		Ok(())
	}

	async fn record(
		&mut self,
		actor: Actor,
		command: &Command,
		outcome: Outcome,
	) -> aidash_application::Result<()> {
		Ok(RunSafetyStop::record(&mut *self.tx, actor, command, outcome).await?)
	}
}

/// Pauses or cancels the Run. The control change, audit entry and receipt
/// commit together, and workers are told only after the commit.
pub(crate) async fn stop(
	f: &Federation,
	actor: Actor,
	id: Uuid,
	input: SafetyStopInput,
	operation: Uuid,
) -> Result<Receipt> {
	let command = Command::new(
		&input.tenant,
		id,
		input.action,
		input.expected_control_revision,
		operation,
		&input.reason,
	)?;
	let scope = target(f, &command.tenant, id).await?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let done = {
		let mut ledger = Stop {
			store: &f.store,
			tx: &mut tx,
			tenant: &command.tenant,
		};
		execute(&mut ledger, actor, &command).await
	};
	let done = match done {
		Ok(done) => done,
		Err(error) => {
			// Nothing written so far may survive a refused or failed command.
			tx.rollback().await?;
			return Err(error.into());
		}
	};
	tx.commit().await?;
	if let Some(action) = done.changed {
		f.store.run_control_committed(id, action.control());
		f.notify.notify_waiters();
	}
	Ok(done.receipt)
}

#[cfg(test)]
#[path = "safety_stop/tests.rs"]
mod tests;
