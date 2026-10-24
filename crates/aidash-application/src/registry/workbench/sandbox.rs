//! Sandbox management applies current draft authority for HTTP and background callers.
use crate::{Error, Result, ports::registry::workbench::sandbox::SandboxRepository};
use aidash_domain::{
	identity::tenant_administration::{COLLECTION, TEST_LIMITS_UPDATE},
	registry::workbench::sandbox::{self, TenantTestLimits, TestLimits, TestSession},
};
use uuid::Uuid;
pub fn validate_limits(repository: &dyn SandboxRepository, limits: &TestLimits) -> Result<()> {
	repository.validate_limit_fields(limits)?;
	sandbox::validate_limits(limits)?;
	Ok(())
}
pub async fn get_limits(repository: &dyn SandboxRepository, id: Uuid) -> Result<TestLimits> {
	let mut scope = repository.begin().await?;
	let draft = scope.draft(id, false).await?;
	scope
		.authorize_draft(&draft, "agent_draft.test", true)
		.await?;
	let value = scope.limits(&draft.tenant).await?;
	scope.commit().await?;
	Ok(value)
}
/// A Tenant's limits as it set them, with the Deployment's ceilings, under
/// `workbench.test_limits.update` (#361).
pub async fn tenant_limits(
	repository: &dyn SandboxRepository,
	tenant: String,
) -> Result<TenantTestLimits> {
	let mut scope = repository.begin().await?;
	scope
		.authorize_tenant(&tenant, TEST_LIMITS_UPDATE, COLLECTION)
		.await?;
	let limits = scope.stored_limits(&tenant).await?;
	scope.commit().await?;
	Ok(TenantTestLimits {
		limits,
		ceilings: repository.ceilings(),
	})
}
/// Sets a Tenant's limits within the Deployment's ceilings, under
/// `workbench.test_limits.update`, at the revision they were read at.
pub async fn set_limits(
	repository: &dyn SandboxRepository,
	tenant: String,
	input: TestLimits,
) -> Result<TenantTestLimits> {
	if tenant != input.tenant {
		return Err(Error::Invalid("tenant mismatch".into()));
	}
	let mut scope = repository.begin().await?;
	scope
		.authorize_tenant(&tenant, TEST_LIMITS_UPDATE, COLLECTION)
		.await?;
	validate_limits(repository, &input)?;
	let ceilings = repository.ceilings();
	ceilings.permits(&input)?;
	let limits = scope.save_limits(&input).await?;
	scope.commit().await?;
	Ok(TenantTestLimits { limits, ceilings })
}
pub async fn sessions(repository: &dyn SandboxRepository, id: Uuid) -> Result<Vec<TestSession>> {
	repository.purge().await?;
	let mut scope = repository.begin().await?;
	let draft = scope.draft(id, false).await?;
	scope
		.authorize_draft(&draft, "agent_draft.read", true)
		.await?;
	let rows = scope.sessions(id).await?;
	scope.commit().await?;
	Ok(rows)
}
pub async fn stop(repository: &dyn SandboxRepository, id: Uuid) -> Result<TestSession> {
	let mut scope = repository.begin().await?;
	let session = scope.session(id, false).await?;
	let draft = scope.draft(session.draft_id, false).await?;
	scope
		.authorize_draft(&draft, "agent_draft.test", true)
		.await?;
	let result = scope.stop(id).await?;
	scope.commit().await?;
	Ok(result)
}
#[cfg(test)]
mod tests;

pub mod dispatch;

pub mod execution;

pub mod admission;

mod deferred;

mod plugins;
