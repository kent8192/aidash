//! The 30 second Read Recheck of running commands and Live previews (ADR 0116).
use super::reconciliation::Repository;
use crate::{
	Error, Result as NativeResult,
	apps::execution::repositories::live_previews,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::{
		read_recheck::{Outcome, ReadRecheckRepository, Target},
		reconciliation::OperationReconciliationRepository as _,
	},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::preview::live::StopReason;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use uuid::Uuid;

async fn check_operation(repository: &Repository<'_>, id: Uuid) -> NativeResult<()> {
	let snapshot = repository.snapshot(id).await?;
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: snapshot.credential_id,
		tenant: snapshot.tenant.clone(),
		subject: snapshot.principal.clone(),
	};
	let mut access = Access::begin(repository.store, &identity).await?;
	access.subjects = serde_json::from_value(snapshot.subjects.clone())?;
	let result = async {
		let mut check = access.rechecking(snapshot.run_id);
		let run = check.recheck_run_reads(snapshot.run_id).await?;
		check.task_read(run.task_id).await.map(|_| ())
	}
	.await;
	access.finish(result).await
}

async fn running_commands(store: &Store) -> NativeResult<Vec<Uuid>> {
	crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("state")).is_in(["prepared", "submitted", "running"]))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(store.pool.in_scope(&TenancyScope::Deployment(
		DeploymentScopeReason::CapabilityMaintenance,
	)))
	.await
}

#[async_trait]
impl ReadRecheckRepository for Repository<'_> {
	async fn targets(&self) -> Result<Vec<Target>> {
		let mut targets: Vec<Target> = running_commands(self.store)
			.await?
			.into_iter()
			.map(Target::Operation)
			.collect();
		targets.extend(
			live_previews::serving(self.store)
				.await?
				.into_iter()
				.map(Target::LivePreview),
		);
		Ok(targets)
	}
	async fn check(&self, target: Target) -> Result<Outcome> {
		let checked = match target {
			Target::Operation(id) => check_operation(self, id).await.map(|()| true),
			Target::LivePreview(id) => live_previews::read_recheck(self.store, id).await,
		};
		match checked {
			Ok(true) => Ok(Outcome::Allowed),
			Ok(false) => Ok(Outcome::Unconfirmed),
			Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
				Ok(Outcome::Refused)
			}
			Err(error) => Err(error.into()),
		}
	}
	async fn stop_refused(&self, target: Target) -> Result<()> {
		match target {
			Target::Operation(id) => {
				let snapshot = self.snapshot(id).await?;
				self.withdraw(&snapshot).await
			}
			Target::LivePreview(id) => {
				live_previews::stop_serving(self.store, id, StopReason::RunAuthorityRevoked)
					.await
					.map_err(Into::into)
			}
		}
	}
	async fn stop_unconfirmed(&self, preview: Uuid) -> Result<()> {
		live_previews::stop_serving(self.store, preview, StopReason::NodeUnreachable)
			.await
			.map_err(Into::into)
	}
}
