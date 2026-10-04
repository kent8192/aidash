//! Paginated generation disclosure always rechecks current subject authority.
use super::visibility;
use crate::{Error, Result, ports::generation::reads::GenerationReads};
use aidash_domain::{
	generation::{
		policy::Spec,
		requests::{History, Request, Usage},
	},
	identity::Principal,
};
use uuid::Uuid;
fn same_tenant(principal: &Principal, tenant: &str) -> Result<()> {
	if let Principal::Subject { tenant: caller, .. } = principal
		&& caller != tenant
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}
pub async fn list(repository: &dyn GenerationReads, tenant: &str) -> Result<Vec<Request>> {
	// The existing ORM lease is acquired before principal dispatch. Operator
	// reads retain one page; subject reads scan until 200 visible rows or EOF.
	let mut pages = repository.pages().await?;
	if matches!(repository.principal(), Principal::Operator) {
		return pages.page(tenant, 0).await;
	}
	same_tenant(repository.principal(), tenant)?;
	let mut scope = repository.begin_subject().await?;
	let result = async {
		let mut visible = vec![];
		let mut offset = 0;
		loop {
			let requests = pages.page(tenant, offset).await?;
			let exhausted = requests.len() < 200;
			for request in requests {
				if visibility::visible(scope.as_mut(), &request).await? {
					visible.push(request);
				}
				if visible.len() == 200 {
					break;
				}
			}
			if exhausted || visible.len() == 200 {
				break;
			}
			offset += 200;
		}
		Ok(visible)
	}
	.await;
	scope.finish_requests(result).await
}
pub async fn history(
	repository: &dyn GenerationReads,
	tenant: &str,
	id: Uuid,
) -> Result<Vec<History>> {
	if matches!(repository.principal(), Principal::Operator) {
		return repository.operator_history(tenant, id).await;
	}
	same_tenant(repository.principal(), tenant)?;
	let mut scope = repository.begin_subject().await?;
	let result = async {
		let job = scope.job(tenant, id).await?.ok_or(Error::Forbidden)?;
		if !visibility::visible(scope.as_mut(), &job).await? {
			return Err(Error::Forbidden);
		}
		scope.history(tenant, id).await
	}
	.await;
	scope.finish_history(result).await
}
pub async fn usage(repository: &dyn GenerationReads, tenant: &str, id: Uuid) -> Result<Usage> {
	if matches!(repository.principal(), Principal::Operator) {
		return repository.operator_usage(tenant, id).await;
	}
	same_tenant(repository.principal(), tenant)?;
	let mut scope = repository.begin_subject().await?;
	let result = async {
		let job = scope.job(tenant, id).await?.ok_or(Error::Forbidden)?;
		if !visibility::visible(scope.as_mut(), &job).await? {
			return Err(Error::Forbidden);
		}
		scope.usage(tenant, id).await
	}
	.await;
	scope.finish_usage(result).await
}
pub async fn specification(
	repository: &dyn GenerationReads,
	tenant: &str,
	id: Uuid,
) -> Result<Spec> {
	if matches!(repository.principal(), Principal::Operator) {
		return Ok(serde_json::from_value(
			repository.operator_specification(tenant, id).await?,
		)?);
	}
	same_tenant(repository.principal(), tenant)?;
	let mut scope = repository.begin_subject().await?;
	let result = async {
		let job = scope.job(tenant, id).await?.ok_or(Error::Forbidden)?;
		if !visibility::visible(scope.as_mut(), &job).await? {
			return Err(Error::Forbidden);
		}
		Ok(serde_json::from_value(
			scope.specification(tenant, id).await?,
		)?)
	}
	.await;
	scope.finish_specification(result).await
}
#[cfg(test)]
mod tests;
