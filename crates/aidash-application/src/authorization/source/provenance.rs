//! Disclosure identity is checked under the original producer's current authority.
use crate::{Error, Result, ports::authorization::source::provenance::SemanticSourceScope};
use aidash_domain::semantic::{Failure, indexing::content_digest};
use uuid::Uuid;
pub async fn verify<S: SemanticSourceScope + ?Sized>(scope: &mut S, grant: Uuid) -> Result<()> {
	let Some(_visit) = scope.source_visit(grant) else {
		return Ok(());
	};
	for source in scope.disclosed_sources(grant).await? {
		let entry = scope
			.disclosed_entry(source.entry_id)
			.await?
			.ok_or(Error::RemoteSemantic(Failure::Invalidated))?;
		if entry.deleted || entry.revision != source.revision {
			return Err(Error::RemoteSemantic(Failure::Invalidated));
		}
		if !scope.source_permitted(&entry).await? {
			return Err(Error::Forbidden);
		}
		let text = scope
			.source_text(entry.workspace_id, &serde_json::from_value(entry.source)?)
			.await?
			.ok_or(Error::Forbidden)?;
		if content_digest(&text) != source.content_digest {
			return Err(Error::RemoteSemantic(Failure::Invalidated));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
