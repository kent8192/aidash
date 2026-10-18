//! Reading one Source citation. A Source citation binds to the Run that
//! published it, so a reference resolves only against that Run's own Evidence
//! fragments, and only for a reader who may read that Run now. Every reason a
//! reader cannot see the evidence is reported as the same absence, so a
//! response never reveals whether the Run or its evidence exists.
//!
//! The readability decision commits before the evidence is read, so a
//! revocation could land in between. The read therefore asks again after the
//! evidence is in hand and returns it only to a reader who may still read the
//! Run, as if the read had stayed inside the authorization. The snapshot is
//! read once more after that decision, so a retention purge committed during
//! the read leaves the fragment without its surrounding context, and the
//! Run's evidence is checked for revocation only after that last read.
//! Authority is decided one final time after all of them.
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
	store
		.snapshot(run, fragment.document_id)
		.await?
		.ok_or_else(unavailable)?;
	// Authority is decided again after the reads.
	if !reader.readable(run).await? {
		return Err(unavailable());
	}
	// Context comes only from lines the snapshot still retains now.
	let document = store
		.snapshot(run, fragment.document_id)
		.await?
		.ok_or_else(unavailable)?;
	// A page image is read with its fragment; it stays after the snapshot is
	// purged, as a passage's text does (ADR 0053).
	let image = match &fragment.image {
		Some(_) => Some(
			store
				.page_image(run, &fragment.reference)
				.await?
				.ok_or_else(unavailable)?
				.1,
		),
		None => None,
	};
	// The evidence itself is decided last, after every read: a revocation
	// committed during any of them withholds what was already read.
	if store.revoked(run).await? {
		return Err(unavailable());
	}
	// Authority is decided once more after every read, the context and the
	// revocation included, so a reader who lost the Run during them gets
	// nothing.
	if !reader.readable(run).await? {
		return Err(unavailable());
	}
	let mut cited = CitedEvidence::new(fragment, &document);
	cited.image = image;
	Ok(cited)
}
