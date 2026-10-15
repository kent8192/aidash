//! Evidence Revocation of all of one Run's Research evidence (ADR 0046).
use crate::{Error, Result, ports::research::RevocationRepository};
use aidash_domain::{
	research::revocation::{self, EvidenceRevocation, REVOKE, Removed},
	retention::{self, Decider},
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Revoke all of one Run's Research evidence. Revocation is irreversible:
/// revoking a Run again returns its first record unchanged.
pub async fn revoke(
	repository: &dyn RevocationRepository,
	run: Uuid,
	reason: &str,
	now: DateTime<Utc>,
) -> Result<EvidenceRevocation> {
	let reason = revocation::reason(reason)?;
	let mut scope = repository.begin().await?;
	let result = async {
		let decider = scope.decider();
		match decider {
			Decider::Subject { .. } => scope.require(run, REVOKE).await?,
			// A Deployment Administrator reads no content to revoke; only the
			// Run's existence is checked.
			Decider::DeploymentAdministrator { .. } => {
				if !scope.run_exists(run).await? {
					return Err(Error::NotFound("run unavailable".into()));
				}
			}
		}
		let tenant = scope.tenant(run).await?;
		scope
			.revoke(&EvidenceRevocation {
				run_id: run,
				tenant,
				revoked_by: decider,
				reason,
				revoked_at: now,
				removed: Removed::default(),
			})
			.await
	}
	.await;
	let revoked = scope.finish(result).await?;
	repository.notify();
	Ok(revoked)
}

/// The revocation records of `tenant`, newest first, for a subject of that
/// Tenant permitted `research.revoke` or a Deployment Administrator.
pub async fn records(
	repository: &dyn RevocationRepository,
	tenant: &str,
	offset: u64,
	limit: u32,
) -> Result<Vec<EvidenceRevocation>> {
	let mut scope = repository.begin_records().await?;
	let result = async {
		if let Decider::Subject { tenant: own, .. } = scope.decider()
			&& own != tenant
		{
			return Err(Error::Forbidden);
		}
		scope.require(tenant, REVOKE).await?;
		scope
			.records(tenant, offset, limit.clamp(1, retention::MAX_PAGE))
			.await
	}
	.await;
	scope.finish(result.is_ok()).await?;
	result
}

#[cfg(test)]
mod tests;
