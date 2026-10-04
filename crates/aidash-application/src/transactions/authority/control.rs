//! Durable transaction authority is checked by every HTTP, peer and recovery caller.
use super::{checks, request, source_checks};
use crate::{
	Error, Result,
	ports::transactions::{
		TransactionAuthorityScope,
		authority::{AuthorityRepository, ControlScope, SubmissionScope},
	},
};
use aidash_domain::{
	identity::execution::ExecutionPrincipal,
	transactions::{
		CoordinatorDecision, CoordinatorTransition, Manifest,
		authority::{Binding, Origin, Preflight, Status},
	},
};
use futures_util::{TryStreamExt, stream};
use serde_json::json;
use uuid::Uuid;

async fn finish<T>(scope: Box<dyn ControlScope>, result: Result<T>) -> Result<T> {
	match result {
		Ok(value) => {
			scope.finish(Ok(())).await?;
			Ok(value)
		}
		Err(error) => {
			scope.finish(Err(error)).await?;
			Err(Error::External("authority scope lost its failure".into()))
		}
	}
}

async fn finish_submission<T>(scope: Box<dyn SubmissionScope>, result: Result<T>) -> Result<T> {
	match result {
		Ok(value) => {
			scope.finish(Ok(())).await?;
			Ok(value)
		}
		Err(error) => {
			scope.finish(Err(error)).await?;
			Err(Error::External("authority scope lost its failure".into()))
		}
	}
}

pub async fn preflight(
	repository: &dyn AuthorityRepository,
	caller: &str,
	input: &Preflight,
) -> Result<()> {
	if !input.permits(caller, repository.node_id()) {
		return Err(Error::Forbidden);
	}
	let mut scope = repository.mapped(input).await?;
	scope.gate().await?;
	let result = async {
		checks(scope.authority(), input, "transaction.submit").await?;
		let authority = scope.authority();
		let binding = Binding {
			request: input.clone(),
			local: Origin::from(&authority.identity()),
			subjects: authority.subjects().to_vec(),
		};
		scope.bind(input.id, &binding).await
	}
	.await;
	finish(scope, result).await
}

pub async fn submit(
	repository: &dyn AuthorityRepository,
	identity: &ExecutionPrincipal,
	manifest: &Manifest,
) -> Result<Status> {
	repository.validate(manifest)?;
	if manifest.coordinator != repository.node_id() {
		return Err(Error::Invalid("submit to the named coordinator".into()));
	}
	let origin = Origin::from(identity);
	for node in &manifest.participants {
		request(manifest, &origin, &node.node_id)?;
	}
	let mut scope = repository.source(&origin).await?;
	if let Err(error) =
		source_checks(scope.authority(), manifest, &origin, "transaction.submit").await
	{
		return finish(scope, Err(error)).await;
	}
	match scope.status(manifest.id).await {
		Ok(existing) => {
			let result = async {
				scope.match_origin(manifest.id, &origin).await?;
				if existing.digest != manifest.digest()? {
					return Err(Error::Conflict("transaction manifest is immutable".into()));
				}
				Ok(existing)
			}
			.await;
			return finish(scope, result).await;
		}
		Err(Error::NotFound(_)) => {}
		Err(error) => return finish(scope, Err(error)).await,
	}
	scope.gate().await?;
	let subjects = scope.authority().subjects().to_vec();
	let mut scope = scope.into_submission()?;
	let result = async {
		// Admission remains uncommitted under the original authority locks until
		// every remote preflight succeeds, including its current policy decision.
		let stored = scope.submit(manifest, &origin).await?;
		let binding = Binding {
			request: request(manifest, &origin, repository.node_id())?,
			local: origin.clone(),
			subjects,
		};
		scope.bind(manifest.id, &binding).await?;
		stream::iter(
			manifest
				.participants
				.iter()
				.filter(|node| node.node_id != repository.node_id())
				.map(Ok::<_, Error>),
		)
		.try_for_each_concurrent(8, |node| {
			let origin = &origin;
			async move {
				let input = request(manifest, origin, &node.node_id)?;
				repository.remote_preflight(&node.node_id, &input).await
			}
		})
		.await?;
		repository
			.fault(manifest.id, "coordinator.submit.before")
			.await?;
		Ok(stored)
	}
	.await;
	let stored = finish_submission(scope, result).await?;
	repository
		.fault(manifest.id, "coordinator.submit.after")
		.await?;
	repository.wake();
	Ok(stored)
}

/// A source attempt survives process loss; only a durable outcome clears it.
pub async fn issue(
	repository: &dyn AuthorityRepository,
	manifest: &Manifest,
	node: &str,
) -> Result<()> {
	let Some(origin) = repository.origin(manifest.id).await? else {
		return Ok(());
	};
	let mut scope = repository.source(&origin).await?;
	let result = async {
		source_checks(scope.authority(), manifest, &origin, "transaction.submit").await?;
		if node != repository.node_id() {
			scope.trusted(node).await?;
		}
		scope.insert_attempt(manifest.id, node).await
	}
	.await;
	repository
		.fault(manifest.id, "authority.issue.before")
		.await?;
	finish(scope, result).await?;
	repository.fault(manifest.id, "authority.issue.after").await
}

pub async fn ticket(
	repository: &dyn AuthorityRepository,
	id: Uuid,
	node: &str,
) -> Result<Preflight> {
	let origin = repository.origin(id).await?.ok_or(Error::Forbidden)?;
	let mut scope = repository.source(&origin).await?;
	let result = async {
		scope.trusted(node).await?;
		let state = scope.status(id).await?;
		if state.decision.is_some() || !scope.pending_attempt(id, node).await? {
			return Err(Error::Forbidden);
		}
		let manifest: Manifest = serde_json::from_value(state.manifest)?;
		source_checks(scope.authority(), &manifest, &origin, "transaction.submit").await?;
		request(&manifest, &origin, node)
	}
	.await;
	let proof = finish(scope, result).await?;
	repository.fault(id, "authority.checked.after").await?;
	Ok(proof)
}

/// The native participant retains the verified access until its protected write.
pub async fn prepare_admission(
	repository: &dyn AuthorityRepository,
	caller: &str,
	manifest: &Manifest,
) -> Result<Option<Binding>> {
	let Some(bound) = repository.binding(manifest.id).await? else {
		if repository.origin(manifest.id).await?.is_some() {
			return Err(Error::Forbidden);
		}
		return Ok(None);
	};
	if bound.request != request(manifest, &bound.request.origin, repository.node_id())?
		|| caller != bound.request.coordinator
	{
		return Err(Error::Forbidden);
	}
	Ok(Some(bound))
}

pub async fn check_admission(
	scope: &mut dyn TransactionAuthorityScope,
	repository: &dyn AuthorityRepository,
	bound: &Binding,
	caller: &str,
) -> Result<()> {
	checks(scope, &bound.request, "transaction.submit").await?;
	if Origin::from(&scope.identity()) != bound.local || scope.subjects() != bound.subjects {
		return Err(Error::Forbidden);
	}
	if caller != repository.node_id()
		&& repository.remote_ticket(caller, bound.request.id).await? != bound.request
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// The immutable owner survives credential rotation and hides other transactions.
pub async fn require_owner(
	repository: &dyn AuthorityRepository,
	identity: &ExecutionPrincipal,
	id: Uuid,
) -> Result<Origin> {
	repository
		.origin(id)
		.await?
		.filter(|origin| origin.tenant == identity.tenant && origin.subject == identity.subject)
		.ok_or_else(|| Error::NotFound("transaction".into()))
}

pub async fn manage(
	repository: &dyn AuthorityRepository,
	identity: &ExecutionPrincipal,
	state: &Status,
	action: &str,
) -> Result<()> {
	let origin = require_owner(repository, identity, state.id).await?;
	let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
	let mut scope = repository.source(&Origin::from(identity)).await?;
	scope.audit(action != "transaction.read");
	let result = async {
		source_checks(scope.authority(), &manifest, &origin, "transaction.read").await?;
		if action != "transaction.read" {
			let authority = scope.authority();
			let resource = authority.resource(
				"transaction",
				state.id,
				json!({
					"coordinator":manifest.coordinator,
					"participants":manifest.participants.iter().map(|p| &p.node_id).collect::<Vec<_>>()
				}),
			);
			authority.require(&resource, action).await?;
		}
		for node in &manifest.participants {
			if node.node_id != repository.node_id() {
				let input = request(&manifest, &origin, &node.node_id)?;
				repository.remote_access(&node.node_id, &input).await?;
			}
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return finish(scope, Err(error)).await;
	}
	let mut scope = scope.into_submission()?;
	let result = async {
		if action == "transaction.abort" {
			scope
				.transition(
					state.id,
					CoordinatorTransition::Decide(CoordinatorDecision::Abort),
					"subject requested abort",
				)
				.await?;
			// The status is read in the same transaction after decision arbitration.
			if scope.status(state.id).await?.decision.as_deref() == Some("COMMIT") {
				return Err(Error::Conflict("commit is irrevocable".into()));
			}
			repository
				.fault(state.id, "coordinator.abort.before")
				.await?;
		}
		Ok(())
	}
	.await;
	finish_submission(scope, result).await?;
	if action == "transaction.abort" {
		repository
			.fault(state.id, "coordinator.abort.after")
			.await?;
	}
	Ok(())
}

pub async fn read_access(
	repository: &dyn AuthorityRepository,
	caller: &str,
	input: &Preflight,
) -> Result<()> {
	if caller != input.coordinator {
		return Err(Error::Forbidden);
	}
	let bound = repository
		.binding(input.id)
		.await?
		.ok_or(Error::Forbidden)?;
	if bound.request != *input {
		return Err(Error::Forbidden);
	}
	let mut scope = repository.mapped(input).await?;
	scope.audit(false);
	if Origin::from(&scope.authority().identity()) != bound.local {
		return Err(Error::Forbidden);
	}
	let result = checks(scope.authority(), input, "transaction.read").await;
	finish(scope, result).await
}

#[cfg(test)]
mod tests;
