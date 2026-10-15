//! A Tenant's Retention Rules (ADR 0045). The Deployment's periods are also
//! the longest a Tenant may keep; a Tenant may store shorter ones.
use crate::{Error, Result, ports::retention::RetentionRepository};
use aidash_domain::retention::{self, Decider, RetentionRules};
use serde::Serialize;

/// The periods that apply to one Tenant and where they come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct TenantRetention {
	pub tenant: String,
	/// The Deployment's periods, also the longest the Tenant may keep.
	pub deployment: RetentionRules,
	/// The Tenant's own shorter periods, if it stores any.
	pub tenant_rules: Option<RetentionRules>,
	/// What applies: the shorter of the two for every period.
	pub effective: RetentionRules,
}

impl TenantRetention {
	fn of(tenant: &str, deployment: RetentionRules, stored: Option<RetentionRules>) -> Self {
		Self {
			tenant: tenant.into(),
			deployment,
			effective: deployment.effective(stored.as_ref()),
			tenant_rules: stored,
		}
	}
}

/// A subject acts only on its own Tenant.
fn own_tenant(decider: &Decider, tenant: &str) -> Result<()> {
	match decider {
		Decider::Subject { tenant: own, .. } if own != tenant => Err(Error::Forbidden),
		_ => Ok(()),
	}
}

/// The periods that apply to `tenant`. Every subject of the Tenant may read them.
pub async fn rules(repository: &dyn RetentionRepository, tenant: &str) -> Result<TenantRetention> {
	let mut scope = repository.begin().await?;
	let result = async {
		own_tenant(&scope.decider(), tenant)?;
		let stored = scope.stored(tenant).await?;
		Ok(TenantRetention::of(tenant, scope.deployment(), stored))
	}
	.await;
	scope.finish(false).await?;
	result
}

/// Store `rules` for `tenant`, or clear them with `None` so the Deployment's
/// periods apply. A Tenant's periods may not exceed the Deployment's.
pub async fn update(
	repository: &dyn RetentionRepository,
	tenant: &str,
	rules: Option<RetentionRules>,
) -> Result<TenantRetention> {
	let mut scope = repository.begin().await?;
	let result = async {
		let decider = scope.decider();
		own_tenant(&decider, tenant)?;
		scope.require(tenant, retention::UPDATE).await?;
		let deployment = scope.deployment();
		if let Some(rules) = &rules {
			rules.validate_for_tenant(&deployment)?;
		}
		// Lock the stored periods so concurrent changes are recorded in order.
		scope.stored(tenant).await?;
		scope.store(tenant, rules.as_ref(), &decider).await?;
		Ok(TenantRetention::of(tenant, deployment, rules))
	}
	.await;
	scope.finish(result.is_ok()).await?;
	result
}

#[cfg(test)]
mod tests;
