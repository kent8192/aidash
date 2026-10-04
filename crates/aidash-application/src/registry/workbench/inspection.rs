//! Agent-version inspection is distinct from execution and global Trust claims.
use crate::{Error, Result, ports::registry::workbench::DraftAuthority};
use aidash_domain::{
	identity::Principal,
	policy::{Evaluation, Resource},
	registry::EntityRef,
};
use serde_json::json;
pub async fn require(scope: &mut dyn DraftAuthority, reference: &EntityRef) -> Result<()> {
	let Principal::Subject { tenant, subject } = scope.principal() else {
		return Ok(());
	};
	scope.lock_identity().await?;
	let decision = scope
		.evaluate(
			&tenant,
			&Evaluation {
				subject,
				action: "agent_version.inspect".into(),
				resource: Resource {
					tenant: tenant.clone(),
					kind: "agent_version".into(),
					id: super::ref_key(reference),
					attributes: json!({"agent_id":reference.id,"version":reference.version}),
				},
				environment: json!({}),
			},
		)
		.await?;
	if decision.allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

#[cfg(test)]
mod tests;
