//! Provenance and status preserve current authority and never expose cached foreign balances.
use crate::{
	Error, Result,
	generation::visibility,
	ports::semantic::remote_status::{StatusRepository, StatusScope},
};
use aidash_domain::semantic::{
	Failure,
	remote::{
		Binding, Receipt,
		status::{Provenance, Status, project},
	},
};
use serde_json::Value;
use uuid::Uuid;
pub async fn provenance(
	scope: &mut dyn StatusScope,
	value: Option<Value>,
) -> Result<Option<Provenance>> {
	let Some(value) = value else { return Ok(None) };
	let mut result = Provenance::from(serde_json::from_value::<Receipt>(value)?);
	let node = scope.node_id().to_owned();
	result.allowance_node = node.clone();
	if let Binding::RequiredHome {
		home_lineage,
		execution_lineage,
		..
	} = &result.binding
	{
		for owner in home_lineage
			.iter()
			.chain(execution_lineage)
			.filter(|owner| owner.node_id == node)
		{
			if owner.tenant != scope.tenant() {
				continue;
			}
			if let Some(job) = scope.request(owner.request_id, &owner.tenant).await?
				&& visibility::visible(scope, &job).await?
			{
				result
					.allowances
					.push(scope.allowance(owner.request_id).await?);
			}
		}
	}
	Ok(Some(result))
}
pub async fn run_receipt(scope: &mut dyn StatusScope, id: Uuid) -> Result<Option<Provenance>> {
	if !scope.run_visible(id).await? {
		return Err(Error::Forbidden);
	}
	let receipt = scope.receipt(id).await?;
	provenance(scope, receipt).await
}
pub async fn load(
	repository: &dyn StatusRepository,
	grant: Uuid,
	binding: &Binding,
	reason: Option<Failure>,
) -> Result<Status> {
	let record = if binding.disabled() {
		None
	} else {
		repository.latest(grant).await?
	};
	Ok(project(binding, reason, record)?)
}
#[cfg(test)]
mod tests;
