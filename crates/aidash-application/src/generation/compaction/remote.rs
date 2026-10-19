//! A RequiredHome Run prunes through the compaction Decider its Home disclosed
//! for this executing Node; each Decision Attempt is admitted against the
//! Home's allowance before it is dispatched and settled once its result is kept.
use crate::{
	Error, Result, authorization::catalog,
	ports::generation::compaction::remote::RemoteCompactionAuthority,
};
use aidash_domain::{
	Run,
	decision::{DeciderConfig, INVOKE},
	federation::execution::Description,
	generation::dispatch::Input,
	registry::{Entry, rules::digest},
	semantic::{Failure, remote::Binding},
};
use uuid::Uuid;

/// The Decider pinned for this executing Node, once its exact version has a
/// Catalog Approval here. `None` when the Home disclosed no Decider.
pub async fn decider(
	authority: &mut dyn RemoteCompactionAuthority,
	run: &Run,
) -> Result<Option<Entry>> {
	authority.suspend().await?;
	if !authority.refresh(run).await? {
		return Err(Error::Forbidden);
	}
	let description: Description = serde_json::from_value(authority.description(run.id).await?)?;
	let Binding::RequiredHome {
		decider: Some(approved),
		..
	} = description.semantic
	else {
		return Ok(None);
	};
	let entry = catalog::entry(authority.catalog(), &approved.entry, INVOKE).await?;
	if approved.node_id != authority.node_id()
		|| entry.kind != "decider"
		|| digest(&serde_json::to_value(&entry)?) != approved.digest
		|| digest(&entry.config) != approved.configuration_digest
	{
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	Ok(Some(entry))
}

/// Admit one Decision Attempt against the Home's allowance at the Decider
/// version's largest request, which the Home can derive from the pinned
/// definition. The reservation stays open while the attempt is dispatched. The
/// local lease is released for the Home round trip and taken again after it,
/// so the attempt's authority is rechecked before dispatch.
pub async fn admit(
	authority: &mut dyn RemoteCompactionAuthority,
	run: &Run,
	attempt: Uuid,
	request_digest: String,
	config: &DeciderConfig,
) -> Result<Input> {
	authority.suspend().await?;
	let amount = i64::try_from(config.max_request_bytes() + 1024)
		.map_err(|_| Error::Invalid("decision request bound overflow".into()))?;
	let reservation = authority
		.admit(run, attempt, request_digest, amount)
		.await?;
	match authority.refresh(run).await {
		Ok(true) => Ok(reservation),
		// Nothing was dispatched, but the Home keeps an admitted reservation
		// open until it is settled, and no caller holds it yet: settle it now.
		refreshed => {
			authority.settle(&reservation).await?;
			refreshed?;
			Err(Error::Forbidden)
		}
	}
}

/// Settle an admitted Decision Attempt once its result is recorded: it reports
/// no usage, and the charge is kept. The local lease is released around the
/// Home round trip, as for its admission.
pub async fn settle(
	authority: &mut dyn RemoteCompactionAuthority,
	run: &Run,
	reservation: &Input,
) -> Result<()> {
	authority.suspend().await?;
	authority.settle(reservation).await?;
	if !authority.refresh(run).await? {
		return Err(Error::Forbidden);
	}
	Ok(())
}

#[cfg(test)]
mod tests;
