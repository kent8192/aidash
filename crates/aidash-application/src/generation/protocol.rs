//! Authenticated allowance RPCs and durable provider admission share the same use cases.
use crate::{
	Error, Result,
	generation::{dispatch, settlement},
	ports::generation::{
		dispatch::{GenerationDispatchRepository, GenerationDispatchSettlement},
		protocol::{
			GenerationProtocolAuthority, GenerationProtocolLease, GenerationProtocolRepository,
		},
		settlement::GenerationSettlementRepository,
	},
};
use aidash_domain::{
	Run,
	context::recovery::Failure as ContextFailure,
	federation::execution::Description,
	generation::{
		dispatch::{FinalizeInput, Input, StepBoundary},
		remote::{Finalization, Purpose, Reserved, Usage, verify_receipts},
	},
	semantic::{
		Failure,
		remote::{
			Binding, Operation,
			reuse::{CitedOperation, Refusal},
		},
	},
};
use serde_json::json;
use uuid::Uuid;

fn exact_provider(description: &Description, usage: &Usage) -> Result<()> {
	match usage.purpose {
		Purpose::Memory => {
			let native = description
				.semantic
				.native()
				.ok_or(Error::RemoteSemantic(Failure::Configuration))?;
			if usage.dispatcher_node != description.source_node
				|| usage.provider.node_id != description.source_node
				|| !native
					.banks
					.iter()
					.any(|bank| bank.roles.contains(&usage.provider))
			{
				return Err(Error::Forbidden);
			}
			// The signed operation does not identify a bank. Enforce every
			// matching bank's cap until that boundary carries an exact bank.
			let cap = native
				.banks
				.iter()
				.filter(|bank| bank.roles.contains(&usage.provider))
				.map(|bank| bank.max_model_tokens)
				.min()
				.ok_or(Error::Forbidden)?;
			if usage.reserved_tokens <= 0
				|| !usize::try_from(usage.reserved_tokens).is_ok_and(|amount| amount <= cap)
			{
				return Err(Error::RemoteSemantic(Failure::Allowance));
			}
		}
		Purpose::Embedding => {
			let Binding::RequiredHome { embedding, .. } = &description.semantic else {
				return Err(Error::RemoteSemantic(Failure::Configuration));
			};
			if **embedding != usage.provider || usage.dispatcher_node != description.source_node {
				return Err(Error::Forbidden);
			}
		}
		Purpose::Compaction => {
			let Binding::RequiredHome {
				compactor: Some(provider),
				..
			} = &description.semantic
			else {
				return Err(Error::RemoteSemantic(Failure::ContextBudget));
			};
			if provider.as_ref() != &usage.provider
				|| usage.dispatcher_node != description.target_node
			{
				return Err(Error::Forbidden);
			}
			let definition = description
				.inspection
				.definitions
				.iter()
				.find(|d| d.kind == "compactor" && d.entry == usage.provider.entry)
				.ok_or(Error::Forbidden)?;
			let config: aidash_domain::registry::CompactorConfig =
				serde_json::from_value(definition.metadata.config.clone())?;
			// Home cannot verify the execution peer's compaction body. Charge
			// the approved maximum instead of trusting its claimed byte count.
			if usage.reserved_tokens != (config.max_request_bytes + 1024) as i64 {
				return Err(Error::Forbidden);
			}
		}
		Purpose::Summary => {
			let provider = description
				.semantic
				.summarizer()
				.ok_or(Error::Context(ContextFailure::SummaryUnavailable))?;
			if provider != &usage.provider || usage.dispatcher_node != description.target_node {
				return Err(Error::Forbidden);
			}
			let definition = description
				.inspection
				.definitions
				.iter()
				.find(|d| d.kind == "model" && d.entry == usage.provider.entry)
				.ok_or(Error::Forbidden)?;
			let config: aidash_domain::model::ModelConfig =
				serde_json::from_value(definition.metadata.config.clone())?;
			let agent: aidash_domain::registry::AgentConfig =
				serde_json::from_value(description.inspection.agent.config.clone())?;
			let policy =
				aidash_domain::context::policy::Effective::of(agent.context_policy.as_ref())
					.summary
					.ok_or(Error::Context(ContextFailure::SummaryUnavailable))?;
			// Home cannot derive the Run's current window, which may cap the
			// summary output further. Charge the approved maximum: one full
			// summarizer request with the policy's output bound.
			let output = policy.max_tokens.min(config.output_token_limit());
			if usage.reserved_tokens != (config.context_window + output as usize) as i64 {
				return Err(Error::Forbidden);
			}
		}
		Purpose::Inference => {
			let agent: aidash_domain::registry::AgentConfig =
				serde_json::from_value(description.inspection.agent.config.clone())?;
			let definition = description
				.inspection
				.definitions
				.iter()
				.find(|d| d.kind == "model" && d.entry == agent.model)
				.ok_or(Error::Forbidden)?;
			if usage.provider.node_id != description.target_node
				|| usage.provider.entry != agent.model
				|| usage.provider.digest != definition.digest
				|| usage.provider.configuration_digest
					!= aidash_domain::registry::rules::digest(&definition.metadata.config)
			{
				return Err(Error::Forbidden);
			}
			let config: aidash_domain::model::ModelConfig =
				serde_json::from_value(definition.metadata.config.clone())?;
			if usage.reserved_tokens
				!= (config.context_window + config.output_token_limit() as usize) as i64
			{
				return Err(Error::Forbidden);
			}
		}
	}
	Ok(())
}

fn refused(refusal: Refusal) -> Error {
	match refusal {
		Refusal::Foreign => Error::Forbidden,
		Refusal::Stale => Error::RemoteSemantic(Failure::Stale),
	}
}

/// A cited operation from this node's journal; an unknown one is foreign.
async fn cited(repository: &dyn GenerationProtocolRepository, id: Uuid) -> Result<CitedOperation> {
	repository
		.semantic_operation(id)
		.await?
		.ok_or(Error::Forbidden)
}

/// The Home accepts a remote Ordered reuse step's citation only if every
/// condition holds; each failure is fail-closed (ADR 0020):
/// - the cited operation belongs to the same Run and grant, is READY, and its
///   step is not after the step being admitted (`Forbidden`);
/// - the grant is still live (`Forbidden`) and no source it consumed changed
///   or was revoked (`Invalidated`, the existing follow-up Task path);
/// - the Workspace corpus digest equals the one recorded before the cited
///   retrieval (`Stale`: the Run retrieves again).
///
/// The Home sees only the step the executing peer claims; that peer's own
/// `verify` pins it to the Run's actual step before the Home admits usage.
async fn accept_reuse(
	authority: &dyn GenerationProtocolAuthority,
	repository: &dyn GenerationProtocolRepository,
	lease: &mut dyn GenerationProtocolLease,
	usage: &Usage,
	boundary: StepBoundary,
	operation: Uuid,
) -> Result<()> {
	let operation = cited(repository, operation).await?;
	operation
		.authorizes(usage.admission_id, usage.grant_id, boundary.step)
		.map_err(refused)?;
	// Checked before the digest: editing a consumed source also changes the
	// corpus, and that edit must reach the follow-up Task path, not a retry.
	lease.semantic_sources_current(usage.grant_id).await?;
	let current = authority.corpus_digest(lease.description()).await?;
	operation.current(&current).map_err(refused)
}

/// The adapter authenticates the peer before passing its node identity here.
pub async fn reserve(
	authority: &dyn GenerationProtocolAuthority,
	repository: &dyn GenerationProtocolRepository,
	source: &str,
	input: Input,
) -> Result<Vec<Reserved>> {
	input.usage.validate()?;
	if source != input.usage.dispatcher_node {
		return Err(Error::Forbidden);
	}
	let mut lease = if matches!(input.usage.purpose, Purpose::Embedding | Purpose::Memory) {
		authority
			.leaf(source, input.usage.grant_id, input.usage.admission_id)
			.await?
	} else {
		authority.grant(source, input.usage.grant_id).await?
	};
	let result = async {
		let description = lease.description().clone();
		exact_provider(&description, &input.usage)?;
		if matches!(input.usage.purpose, Purpose::Embedding | Purpose::Memory) {
			let operation: Operation = serde_json::from_value(input.boundary.clone())?;
			operation.validate()?;
			if operation.id != input.usage.operation_id
				|| operation.digest()? != input.usage.input_digest
				|| operation.grant_id != input.usage.grant_id
				|| operation.admission_id != input.usage.admission_id
				|| operation.home_node != source
				|| (input.usage.purpose == Purpose::Embedding
					&& input.usage.reserved_tokens != (operation.query.len() + 1024) as i64)
			{
				return Err(Error::Forbidden);
			}
			authority.verify_semantic(&description, &operation).await?;
		} else {
			if lease.binding_admission(input.usage.grant_id).await?
				!= Some(input.usage.admission_id)
			{
				return Err(Error::Forbidden);
			}
			let boundary: StepBoundary =
				serde_json::from_value(input.boundary.clone()).map_err(|_| Error::Forbidden)?;
			if let Some(operation) = boundary.reuses {
				accept_reuse(
					authority,
					repository,
					lease.as_mut(),
					&input.usage,
					boundary,
					operation,
				)
				.await?;
			}
			if !authority.verify_peer(source, &input).await? {
				return Err(Error::Forbidden);
			}
		}
		lease.reserve(&input.usage).await
	}
	.await;
	lease.finish_reservations(result).await
}

pub async fn verify(
	authority: &dyn GenerationProtocolAuthority,
	repository: &dyn GenerationProtocolRepository,
	dispatch_repository: &dyn GenerationDispatchRepository,
	source: &str,
	input: Input,
) -> Result<bool> {
	if input.usage.dispatcher_node != authority.node_id()
		|| matches!(input.usage.purpose, Purpose::Embedding | Purpose::Memory)
	{
		return Err(Error::Forbidden);
	}
	let lease = authority
		.leaf(source, input.usage.grant_id, input.usage.admission_id)
		.await?;
	let description = lease.description().clone();
	let result = async {
		exact_provider(&description, &input.usage)?;
		let record = dispatch::bound(dispatch_repository, &input).await?;
		let run = repository.run(input.usage.admission_id).await?;
		let boundary: StepBoundary =
			serde_json::from_value(input.boundary.clone()).map_err(|_| Error::Forbidden)?;
		if record.peer_node != source || record.state != "PREPARING" || boundary.step != run.step {
			return Err(Error::Forbidden);
		}
		if description.semantic.disabled() {
			return Ok(true);
		}
		match boundary.reuses {
			// A retrieval-reuse step: the record bound above shows that this node
			// cited the operation itself; its journal must hold it READY for this
			// Run and grant at or before the Run's actual step.
			Some(operation) => cited(repository, operation)
				.await?
				.authorizes(run.id, input.usage.grant_id, run.step)
				.map_err(refused)?,
			None if repository
				.semantic_ready(run.id, input.usage.grant_id, run.step)
				.await? => {}
			None => return Err(Error::RemoteSemantic(Failure::Pending)),
		}
		Ok(true)
	}
	.await;
	lease.finish_verification(result).await
}

pub async fn finalize(
	repository: &dyn GenerationSettlementRepository,
	source: &str,
	input: FinalizeInput,
) -> Result<bool> {
	if input.usage.dispatcher_node != source {
		return Err(Error::Forbidden);
	}
	match settlement::finalize(repository, &input.usage, &input.result).await {
		Ok(()) | Err(Error::RemoteSemantic(Failure::ProviderContract)) => Ok(true),
		Err(error) => Err(error),
	}
}

pub struct Admission<'a> {
	pub run: &'a Run,
	pub attempt: Uuid,
	pub purpose: Purpose,
	pub input_digest: String,
	pub amount: i64,
}

/// The caller suspends its worker lease before any Home RPC. Local reservation
/// reacquires fresh authority after verifying the Home's exact lineage receipts.
pub async fn admit(
	authority: &dyn GenerationProtocolAuthority,
	repository: &dyn GenerationProtocolRepository,
	dispatch_repository: &dyn GenerationDispatchRepository,
	delivery: &dyn GenerationDispatchSettlement,
	request: Admission<'_>,
) -> Result<Input> {
	let run = request.run;
	let description = repository.description(run.id, &run.home_node).await?;
	let provider = match request.purpose {
		Purpose::Inference => {
			let agent: aidash_domain::registry::AgentConfig =
				serde_json::from_value(description.inspection.agent.config.clone())?;
			let definition = description
				.inspection
				.definitions
				.iter()
				.find(|definition| definition.kind == "model" && definition.entry == agent.model)
				.ok_or(Error::Forbidden)?;
			aidash_domain::semantic::remote::Provider {
				node_id: authority.node_id().into(),
				entry: agent.model,
				digest: definition.digest.clone(),
				configuration_digest: aidash_domain::registry::rules::digest(
					&definition.metadata.config,
				),
			}
		}
		Purpose::Compaction => match &description.semantic {
			Binding::RequiredHome {
				compactor: Some(provider),
				..
			} => (**provider).clone(),
			_ => return Err(Error::RemoteSemantic(Failure::ContextBudget)),
		},
		Purpose::Summary => description
			.semantic
			.summarizer()
			.cloned()
			.ok_or(Error::Context(ContextFailure::SummaryUnavailable))?,
		Purpose::Embedding | Purpose::Memory => return Err(Error::Forbidden),
	};
	// A step without a READY retrieval of its own reuses an earlier one
	// (ADR 0020) and cites the operation whose receipt it carries.
	let reuses = if description.semantic.disabled()
		|| repository
			.semantic_ready(run.id, description.grant_id, run.step)
			.await?
	{
		None
	} else {
		repository
			.run(run.id)
			.await?
			.context
			.source_observation
			.as_ref()
			.and_then(|observation| observation.semantic_operation())
	};
	let input = Input {
		usage: Usage {
			operation_id: request.attempt,
			attempt_id: request.attempt,
			dispatcher_node: authority.node_id().into(),
			grant_id: description.grant_id,
			admission_id: run.id,
			purpose: request.purpose,
			provider,
			input_digest: request.input_digest,
			reserved_tokens: request.amount,
		},
		boundary: json!(StepBoundary {
			step: run.step,
			reuses,
		}),
	};
	dispatch::prepare(dispatch_repository, &input, &run.home_node).await?;
	let result: Result<()> = async {
		let mut receipts = authority.reserve_peer(&run.home_node, &input).await?;
		if let Binding::RequiredHome { home_lineage, .. } = &description.semantic {
			verify_receipts(home_lineage, &receipts, &input.usage)?;
		}
		let mut lease = authority.worker(run).await?.ok_or(Error::Forbidden)?;
		let local = lease.reserve(&input.usage).await;
		receipts.extend(lease.finish_reservations(local).await?);
		dispatch::admitted(dispatch_repository, &input, &receipts).await
	}
	.await;
	if let Err(error) = result {
		let _ = dispatch::finish(
			dispatch_repository,
			delivery,
			&input,
			Finalization::Aborted {},
		)
		.await;
		return Err(error);
	}
	Ok(input)
}

#[cfg(test)]
mod tests;
