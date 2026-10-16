//! Reading one Source citation. A Source citation binds to the Run that
//! published it, so a reference resolves only against that Run's own Evidence
//! fragments, and only for a reader who may read that Run now. Every reason a
//! reader cannot see the evidence is reported as the same absence, so a
//! response never reveals whether the Run or its evidence exists.
//!
//! The readability decision commits before the evidence is read, so a
//! revocation could land in between. The read therefore asks again after the
//! evidence is in hand and returns it only to a reader who may still read the
//! Run, as if the read had stayed inside the authorization.
use crate::{
	Error, Result,
	ports::research::{EvidenceStore, RunReader},
};
use aidash_domain::research::{citations::EvidenceRef, evidence::CitedEvidence};
use uuid::Uuid;

/// The one answer for evidence the reader cannot see, whatever the reason.
pub fn unavailable() -> Error {
	Error::NotFound("evidence unavailable".into())
}

/// The Evidence fragment `reference` of the Run `run`, with its Document
/// snapshot provenance, for a reader `reader` decides may read that Run.
pub async fn read(
	reader: &dyn RunReader,
	store: &dyn EvidenceStore,
	run: Uuid,
	reference: &str,
) -> Result<CitedEvidence> {
	let reference = reference
		.parse::<EvidenceRef>()
		.map_err(|_| unavailable())?;
	if !reader.readable(run).await? {
		return Err(unavailable());
	}
	let fragment = store
		.fragments(run, &[reference])
		.await?
		.into_iter()
		.next()
		.ok_or_else(unavailable)?;
	let document = store
		.snapshot(run, fragment.document_id)
		.await?
		.ok_or_else(unavailable)?;
	// Authority, and the evidence itself, are decided again after the reads:
	// a revocation committed meanwhile withholds what was already read.
	if !reader.readable(run).await? || store.revoked(run).await? {
		return Err(unavailable());
	}
	Ok(CitedEvidence::new(fragment, &document))
}
