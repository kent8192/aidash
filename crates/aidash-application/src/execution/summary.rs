//! Summary Stage authorization: only the exact pinned summarizer may run, and
//! only after every approval and allowance holds. No provider is substituted.
use crate::{Error, Result, ports::execution::summary::SummaryAuthorization};
use aidash_domain::{
	context::{recovery::Failure, summary::SummaryProvider},
	registry::rules::digest,
	semantic::remote::{Binding, Provider},
};

/// Approval and allowance failures become the typed Summary Stage failure;
/// storage and transport failures keep their own classification.
pub fn classify(error: Error) -> Error {
	match error {
		Error::Forbidden
		| Error::Unauthorized
		| Error::NotFound(_)
		| Error::Invalid(_)
		| Error::Conflict(_)
		| Error::RemoteSemantic(_)
		| Error::Context(_) => Error::Context(Failure::SummaryUnavailable),
		other => other,
	}
}

/// The Home-disclosed pin must name the exact summarizer definition run by this node.
pub fn remote_pin<'a>(
	binding: &'a Binding,
	summarizer: &SummaryProvider,
	node: &str,
) -> Result<&'a Provider> {
	binding
		.summarizer()
		.filter(|pin| {
			pin.entry == summarizer.model
				&& pin.digest == summarizer.definition_digest
				&& pin.node_id == node
		})
		.ok_or(Error::Context(Failure::SummaryUnavailable))
}

/// Authorize one Summary Stage request and charge generated allowances before I/O.
pub async fn authorize(
	port: &dyn SummaryAuthorization,
	summarizer: &SummaryProvider,
	request_bytes: i64,
) -> Result<()> {
	async {
		port.refresh().await?;
		if port.remote() {
			let binding = port.remote_binding().await?;
			remote_pin(&binding, summarizer, port.node_id())?;
			return Ok(());
		}
		port.catalog_entry(&summarizer.model, "registry.read")
			.await?;
		let entry = port.catalog_entry(&summarizer.model, "model.infer").await?;
		if entry.kind != "model"
			|| digest(&serde_json::to_value(&entry)?) != summarizer.definition_digest
		{
			return Err(Error::Context(Failure::SummaryUnavailable));
		}
		port.charge_generated(summarizer, request_bytes).await
	}
	.await
	.map_err(classify)
}

#[cfg(test)]
mod tests;
