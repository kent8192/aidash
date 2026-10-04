//! Remote compaction pins the exact provider and preserves allowance/lease ordering.
use crate::{
	Error, Result,
	authorization::catalog,
	ports::{
		CompactionQuestions,
		generation::compaction::remote::{RemoteCompactionAuthority, RemoteCompactionProvider},
	},
};
use aidash_domain::{
	Run,
	federation::execution::Description,
	registry::{CompactorConfig, rules::digest},
	semantic::{Failure, remote::Binding},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub async fn ask(
	authority: &mut dyn RemoteCompactionAuthority,
	provider: &dyn RemoteCompactionProvider,
	run: &Run,
	state: &Value,
	questions: &CompactionQuestions,
) -> Result<Value> {
	authority.suspend().await?;
	if !authority.refresh(run).await? {
		return Err(Error::Forbidden);
	}
	let description: Description = serde_json::from_value(authority.description(run.id).await?)?;
	let Binding::RequiredHome {
		compactor: Some(approved),
		..
	} = description.semantic
	else {
		return Err(Error::RemoteSemantic(Failure::ContextBudget));
	};
	let entry = catalog::entry(authority.catalog(), &approved.entry, "compaction.invoke").await?;
	if approved.node_id != authority.node_id()
		|| entry.kind != "compactor"
		|| digest(&serde_json::to_value(&entry)?) != approved.digest
		|| digest(&entry.config) != approved.configuration_digest
	{
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	let config: CompactorConfig = serde_json::from_value(entry.config)
		.map_err(|_| Error::RemoteSemantic(Failure::Configuration))?;
	let reserved_tokens = (config.max_request_bytes + 1024) as i64;
	let transport = provider
		.approved_remote(config)
		.map_err(|_| Error::RemoteSemantic(Failure::Configuration))?;
	transport.check_credential()?;
	transport
		.check_request(state, questions)
		.map_err(|_| Error::RemoteSemantic(Failure::ContextBudget))?;
	authority.suspend().await?;
	let reservation = authority
		.admit(
			run,
			Uuid::new_v4(),
			digest(&json!({"state":state,"questions":questions})),
			reserved_tokens,
		)
		.await?;
	let response = transport.ask(state, questions).await;
	authority.settle(&reservation).await?;
	if !authority.refresh(run).await? {
		return Err(Error::Forbidden);
	}
	response
}

#[cfg(test)]
mod tests;
