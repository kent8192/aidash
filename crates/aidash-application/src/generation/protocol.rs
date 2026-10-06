//! Authenticated allowance RPCs and durable provider admission share the same use cases.
use crate::{
	Error, Result,
	generation::{dispatch, settlement},
	ports::generation::{
		dispatch::{GenerationDispatchRepository, GenerationDispatchSettlement},
		protocol::{GenerationProtocolAuthority, GenerationProtocolRepository},
		settlement::GenerationSettlementRepository,
	},
};
use aidash_domain::{
	Run,
	federation::execution::Description,
	generation::{
		dispatch::{FinalizeInput, Input},
		remote::{Finalization, Purpose, Reserved, Usage, verify_receipts},
	},
	semantic::{
		Failure,
		remote::{Binding, Operation},
	},
};
use serde_json::{Value, json};
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
			if !native.banks.iter().any(|bank| {
				bank.roles.contains(&usage.provider)
					&& usage.reserved_tokens > 0
					&& usize::try_from(usage.reserved_tokens)
						.is_ok_and(|amount| amount <= bank.max_model_tokens)
			}) {
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

/// The adapter authenticates the peer before passing its node identity here.
pub async fn reserve(
	authority: &dyn GenerationProtocolAuthority,
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
		if record.peer_node != source
			|| record.state != "PREPARING"
			|| input.boundary.get("step").and_then(Value::as_i64) != Some(run.step as i64)
		{
			return Err(Error::Forbidden);
		}
		if !description.semantic.disabled()
			&& !repository
				.semantic_ready(run.id, input.usage.grant_id, run.step)
				.await?
		{
			return Err(Error::RemoteSemantic(Failure::Pending));
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
		Purpose::Embedding | Purpose::Memory => return Err(Error::Forbidden),
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
		boundary: json!({"step":run.step}),
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
