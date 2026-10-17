//! A generated chain prunes through the compaction Decider every generated
//! ancestor's policy names, and each Decision Attempt charges every ancestor.
use crate::{
	Error, Result,
	authorization::catalog,
	ports::generation::compaction::{
		GenerationCompactionAuthority, GenerationCompactionRepository,
	},
};
use aidash_domain::{
	context::recovery::Failure,
	decision::INVOKE,
	generation::{
		compaction::{Attempt, Context},
		policy::Spec,
	},
	registry::{EntityRef, Entry},
};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Which compaction Decider a Run may use, by where it executes.
#[derive(Debug)]
pub enum Chain {
	/// Outside a generated chain: the Agent's own Binding decides.
	Ordinary,
	/// In a generated chain or a RequiredHome execution: only this Decider,
	/// or none, whatever the Agent's Binding names (ADR 0042).
	Generated(Option<Box<Entry>>),
}

/// The generated ancestors and the Decider version all of their policies name.
/// `None` for a generated chain whose ancestors name none or different versions:
/// such a Run is not pruned (ADR 0042).
fn agreed(jobs: &[(Uuid, serde_json::Value)]) -> Result<Option<(Vec<Uuid>, EntityRef)>> {
	let mut reference: Option<EntityRef> = None;
	for (_, document) in jobs {
		let spec: Spec = serde_json::from_value(document.clone())?;
		let Some(approved) = spec.compaction else {
			return Ok(None);
		};
		if reference
			.as_ref()
			.is_some_and(|reference| *reference != approved.provider)
		{
			return Ok(None);
		}
		reference = Some(approved.provider);
	}
	Ok(reference.map(|reference| (jobs.iter().map(|(id, _)| *id).collect(), reference)))
}

/// The compaction Decider of a Run in a generated chain, once the Run is live
/// and the exact Decider version has its Catalog Approval (`decision.invoke`).
pub async fn decider(
	scope: &mut dyn GenerationCompactionAuthority,
	node: &str,
	context: &Context,
) -> Result<Chain> {
	let jobs = scope.ancestors(node).await?;
	if jobs.is_empty() {
		return Ok(Chain::Ordinary);
	}
	let Some((_, reference)) = agreed(&jobs)? else {
		return Ok(Chain::Generated(None));
	};
	super::publication::require_live(scope.live(), node, context.task, &context.agent).await?;
	catalog::entry(scope.catalog(), &reference, "registry.read").await?;
	let entry = catalog::entry(scope.catalog(), &reference, INVOKE).await?;
	if entry.kind != "decider" {
		return Err(Error::Forbidden);
	}
	Ok(Chain::Generated(Some(Box::new(entry))))
}

/// Charge one call and record the Decision Attempt for every generated
/// ancestor in one transaction, before the attempt is dispatched. The ledger
/// keeps the Decider version's largest request rather than the request itself.
/// Returns one owner receipt per ancestor. Failed, uncertain and crashed
/// attempts stay charged.
#[allow(clippy::too_many_arguments)]
pub async fn charge(
	scope: &mut dyn GenerationCompactionAuthority,
	repository: &dyn GenerationCompactionRepository,
	context: &Context,
	decider: &EntityRef,
	attempt: Uuid,
	request_bytes: i64,
	questions: i32,
) -> Result<BTreeMap<String, Uuid>> {
	// An ancestor whose policy no longer names this Decider withdrew its approval.
	let jobs = scope.ancestors(repository.node_id()).await?;
	let Some((jobs, reference)) = agreed(&jobs)?.filter(|(_, reference)| reference == decider)
	else {
		return Err(Error::Context(Failure::DeciderUnapproved));
	};
	super::publication::require_live(
		scope.live(),
		repository.node_id(),
		context.task,
		&context.agent,
	)
	.await?;
	let record = Attempt {
		id: attempt,
		run: context.run,
		provider: reference,
		request_bytes,
		questions,
	};
	let mut transaction = repository.begin().await?;
	let mut receipts = BTreeMap::new();
	for id in jobs {
		if !transaction.charge(id).await? {
			return Err(Error::Context(Failure::DeciderCallsExhausted));
		}
		transaction.reserve(id, &record).await?;
		receipts.insert(format!("{}/generation/{id}", repository.node_id()), attempt);
	}
	transaction.commit().await?;
	Ok(receipts)
}

#[cfg(test)]
mod tests;

pub mod remote;
