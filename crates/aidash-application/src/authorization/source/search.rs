//! Home retrieval retains dispatch fences, current source leases and post-result revocation checks.
use crate::{
	Error, Result,
	generation::dispatch,
	ports::authorization::source::search::{SemanticSearchRepository, SemanticSearchScope},
	semantic::remote_journal as journal,
};
use aidash_domain::{
	generation::{dispatch::Input, remote},
	qualified_agent,
	registry::{AgentConfig, rules::digest},
	semantic::{
		Failure,
		indexing::content_digest,
		remote::{Binding, Operation, Receipt, bounded_query, journal::Claim},
		retrieval::Search,
	},
};
use serde_json::json;
pub fn failure(error: &Error) -> Failure {
	match error {
		Error::RemoteSemantic(reason) => *reason,
		Error::Forbidden | Error::Unauthorized => Failure::Authority,
		Error::Invalid(_)
		| Error::NotFound(_)
		| Error::Domain(aidash_domain::Error::Invalid(_)) => Failure::Configuration,
		_ => Failure::Unavailable,
	}
}
pub async fn search<R: SemanticSearchRepository + ?Sized>(
	repository: &R,
	node: &str,
	operation: Operation,
) -> Result<Receipt> {
	operation.validate()?;
	if operation.home_node != repository.home_node_id() {
		return Err(Error::Forbidden);
	}
	let (mut access, description) = repository.description(node, operation.grant_id).await?;
	let result = Box::pin(async {
		if description.semantic.disabled() {
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
		let bound = access
			.admission_binding(operation.grant_id)
			.await?
			.ok_or(Error::Forbidden)?;
		if bound.admission_id != operation.admission_id {
			return Err(Error::Forbidden);
		}
		access.disclosure_mode(operation.grant_id)?;
		let task = access.disclosure_task(description.task.id).await?;
		let mut query = format!("{}\n{}", task.title, task.description);
		for input in &operation.inputs {
			let message = access.input_message(task.workspace_id, input.id).await?;
			if content_digest(&message.content) != input.digest {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
			query.push('\n');
			query.push_str(&message.content);
		}
		if bounded_query(&query, 32768).0 != operation.query
			|| operation.boundary.inputs_digest != digest(&json!(operation.inputs))
			|| task.revision != operation.boundary.task_revision
		{
			return Err(Error::Forbidden);
		}
		let receiver = repository.verify_operation(node, &operation).await?;
		if !receiver {
			return Err(Error::Forbidden);
		}
		journal::prepare(repository.journal(), &operation, &description.semantic).await?;
		let agent: AgentConfig =
			serde_json::from_value(description.inspection.agent.config.clone())?;
		let spec = access.index_spec(task.workspace_id).await?;
		let transport_truncated = query.len() > 32768;
		let (query, query_truncated) = bounded_query(&operation.query, spec.max_input_bytes);
		let max_tokens = operation.max_tokens.min(spec.max_result_tokens);
		let input = Search {
			query,
			agent: Some(qualified_agent(
				node,
				&description.inspection.agent.id,
				&description.inspection.agent.version,
			)),
			metadata: operation.metadata.clone(),
			limit: spec.max_results,
			max_tokens,
		};
		let prepared = access
			.prepare_search(task.workspace_id, &input, &agent)
			.await?;
		let candidate_digest = prepared.candidate_digest();
		let claim = journal::claim(repository.journal(), operation.id).await?;
		let attempt = match claim {
			Claim::Ready(receipt) if receipt.candidate_digest == candidate_digest => {
				return Ok(*receipt);
			}
			Claim::Ready(_) => {
				repository.journal().expire_cached(operation.id).await?;
				match journal::claim(repository.journal(), operation.id).await? {
					Claim::Attempt(attempt) => attempt,
					Claim::Ready(_) => {
						return Err(Error::RemoteSemantic(Failure::Pending));
					}
				}
			}
			Claim::Attempt(attempt) => attempt,
		};
		let mut dispatch_input = None;
		let result: Result<Receipt> = async {
			let result = if prepared.allowed.is_empty() {
				prepared.result
			} else {
				prepared.check_points(repository.vector()).await?;
				let Binding::RequiredHome {
					embedding,
					execution_lineage,
					..
				} = &description.semantic
				else {
					return Err(Error::Forbidden);
				};
				let admission = Input {
					usage: remote::Usage {
						operation_id: operation.id,
						attempt_id: attempt.id,
						dispatcher_node: repository.home_node_id().to_owned(),
						grant_id: operation.grant_id,
						admission_id: operation.admission_id,
						purpose: remote::Purpose::Embedding,
						provider: (**embedding).clone(),
						input_digest: operation.digest()?,
						reserved_tokens: (input.query.len() + 1024) as i64,
					},
					boundary: json!(operation),
				};
				dispatch::prepare(repository.dispatch(), &admission, node).await?;
				dispatch_input = Some(admission.clone());
				let mut reservations = repository.receiver_reservations(node, &admission).await?;
				remote::verify_receipts(execution_lineage, &reservations, &admission.usage)?;
				reservations.extend(access.reserve_home(&admission.usage).await?);
				dispatch::admitted(repository.dispatch(), &admission, &reservations).await?;
				journal::dispatched(repository.journal(), &attempt, &json!(reservations)).await?;
				let embedding = repository
					.embedding()
					.embed(&spec.embedding, &input.query)
					.await?;
				dispatch::finish(
					repository.dispatch(),
					repository.settlement(),
					&admission,
					remote::Finalization::Settled {
						reported: embedding.tokens.and_then(|n| i64::try_from(n).ok()),
					},
				)
				.await?;
				if embedding
					.tokens
					.is_some_and(|n| n > (input.query.len() + 1024) as u64)
				{
					return Err(Error::RemoteSemantic(Failure::ProviderContract));
				}
				access.finish_search(prepared, &embedding.vector).await?
			};
			let mut receipt = Receipt {
				operation_id: operation.id,
				operation_digest: operation.digest()?,
				home_node: repository.home_node_id().to_owned(),
				tenant: description.source_tenant.clone(),
				workspace_id: task.workspace_id,
				grant_id: operation.grant_id,
				admission_id: operation.admission_id,
				executor: input.agent.clone().ok_or(Error::Forbidden)?,
				binding: description.semantic.clone(),
				retrieved_at: chrono::Utc::now(),
				query_truncated: query_truncated || transport_truncated,
				candidate_digest,
				sources: vec![],
				result,
				estimated_tokens: 0,
			};
			receipt.fit_budget(max_tokens)?;
			journal::complete(repository.journal(), &attempt, &receipt).await?;
			Ok(receipt)
		}
		.await;
		match result {
			Ok(receipt) => Ok(receipt),
			Err(error) => {
				let reason =
					journal::failed(repository.journal(), &attempt, failure(&error)).await?;
				if let Some(input) = dispatch_input {
					let _ = dispatch::finish(
						repository.dispatch(),
						repository.settlement(),
						&input,
						remote::Finalization::Aborted {},
					)
					.await;
				}
				Err(Error::RemoteSemantic(reason))
			}
		}
	})
	.await;
	let receipt = access.finish(result).await?;
	// Release pre-dispatch leases and reacquire after the response. A queued
	// revocation must be observed before source text leaves the Home node.
	let (access, _) = repository.description(node, operation.grant_id).await?;
	access.finish(Ok(receipt)).await
}

#[cfg(test)]
mod tests;
