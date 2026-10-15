//! Evidence Revocation of all of one Run's Research evidence (ADR 0046).
use crate::{Error, Result, ports::research::RevocationRepository};
use aidash_domain::{
	research::revocation::{self, EvidenceRevocation, REVOKE, Removed},
	retention::Decider,
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

#[cfg(test)]
mod tests;
