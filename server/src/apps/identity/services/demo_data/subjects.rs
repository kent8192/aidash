//! Step 2 of a Demo data load: the fictional subjects of a Tenant's policy.
use crate::Result;
use crate::apps::identity::models::AuthorizationBundle;
use crate::apps::identity::repositories::tenant_deletions;
use crate::database::native::Pool;
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::tenant_administration::add_user_subject;
use std::collections::BTreeSet;

/// The actor the policy revision records, as for a Deployment Administrator's
/// edit: the command acts with Deployment authority.
const ACTOR: &str = "demo-data";

/// A user subject and the Assignable Groups it joins.
pub(super) type Wanted = (String, BTreeSet<String>);

/// Adds every subject in `wanted` that the Tenant's policy lacks, in one
/// policy revision and only when something is missing. A subject that already
/// exists is left as it is, whoever changed it since. Returns `false`, writing
/// nothing, when the Tenant is deleted: a reviewer's deletion stays in force.
pub(super) async fn add(pool: &Pool, tenant: &str, wanted: &[Wanted]) -> Result<bool> {
	pool.scoped(&TenancyScope::tenant(tenant)?, async |tx| {
		if tenant_deletions::is_deleted(&mut **tx, tenant).await? {
			return Ok(false);
		}
		let mut snapshot = AuthorizationBundle::lock_snapshot(&mut **tx, tenant, true).await?;
		let mut changed = false;
		for (subject, groups) in wanted {
			if snapshot.bundle.subjects.contains_key(subject) {
				continue;
			}
			add_user_subject(&mut snapshot.bundle, subject, groups)?;
			changed = true;
		}
		if changed {
			AuthorizationBundle::replace(
				&mut **tx,
				tenant,
				snapshot.revision,
				serde_json::to_value(&snapshot.bundle)?,
				ACTOR,
				None,
			)
			.await?;
		}
		Ok(true)
	})
	.await
}
