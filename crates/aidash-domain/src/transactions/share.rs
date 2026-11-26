//! What one participant receives: its own part, sealed, and every part's seal
//! (ADR 0146, #1107).
//!
//! The coordinator holds the submitted [`Manifest`]. A participant holds a
//! [`Share`]: the transaction header, every participant's [`Seal`] and only its
//! own [`Part`]. A seal binds a part through a hash over the part and a random
//! value only that participant receives, so another participant cannot recover a
//! short or guessable value by trying candidates. The transaction digest covers
//! the header and every seal, so all participants agree on every part without
//! seeing it.
//!
//! A seal also lists claims: one hash per Task completion and per Run
//! finalization, naming the Task. A participant checks a paired obligation
//! (a delegated completion needs the executor's Run finalization, and the
//! reverse) against the counterpart's claims. Each owner checks that its own
//! claims match its mutations, and the digest makes everyone hold the same seals.
use super::{Isolation, Manifest, Mutation, validate_mutations, validate_nodes};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// One participant's mutations with the random value that seals them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionPart")]
#[serde(deny_unknown_fields)]
pub struct Part {
	pub node_id: String,
	/// 32 random bytes in lowercase hex, known to the coordinator and this
	/// participant only.
	pub nonce: String,
	pub mutations: Vec<Mutation>,
}

/// What every participant learns about one part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionSeal")]
#[serde(deny_unknown_fields)]
pub struct Seal {
	pub node_id: String,
	/// SHA-256 of the part's JSON encoding.
	pub hash: String,
	/// Sorted [`claim`] hashes of the part's Task completions and Run
	/// finalizations.
	pub claims: Vec<String>,
}

/// One participant's view of a transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionShare")]
#[serde(deny_unknown_fields)]
pub struct Share {
	pub id: Uuid,
	pub coordinator: String,
	pub isolation: Isolation,
	pub deadline: DateTime<Utc>,
	/// Every participant's seal, in participant order.
	pub seals: Vec<Seal>,
	pub part: Part,
}

/// What the digest covers: everything but the part itself.
#[derive(Serialize)]
struct Header<'a> {
	id: &'a Uuid,
	coordinator: &'a str,
	isolation: &'a Isolation,
	deadline: &'a DateTime<Utc>,
	seals: &'a [Seal],
}

fn sha256(bytes: impl AsRef<[u8]>) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

/// The claim a part makes for one Task: `complete_task` or `finish_run`.
pub fn claim(kind: &str, task: Uuid) -> String {
	sha256(format!("{kind}:{task}"))
}

fn claims(mutations: &[Mutation]) -> Vec<String> {
	let mut claims = mutations
		.iter()
		.filter_map(|mutation| match mutation {
			Mutation::CompleteTask { task_id, .. } => Some(claim("complete_task", *task_id)),
			Mutation::FinishRun { task_id, .. } => Some(claim("finish_run", *task_id)),
			Mutation::ThreadTitle { .. } => None,
		})
		.collect::<Vec<_>>();
	claims.sort();
	claims
}

impl Part {
	pub fn seal(&self) -> Result<Seal> {
		Ok(Seal {
			node_id: self.node_id.clone(),
			hash: sha256(serde_json::to_vec(self)?),
			claims: claims(&self.mutations),
		})
	}
}

fn digest(
	id: &Uuid,
	coordinator: &str,
	isolation: &Isolation,
	deadline: &DateTime<Utc>,
	seals: &[Seal],
) -> Result<String> {
	Ok(sha256(serde_json::to_vec(&Header {
		id,
		coordinator,
		isolation,
		deadline,
		seals,
	})?))
}

fn is_hex(value: &str) -> bool {
	value.len() == 64
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Share {
	/// The transaction digest every participant and the coordinator agree on.
	pub fn digest(&self) -> Result<String> {
		digest(
			&self.id,
			&self.coordinator,
			&self.isolation,
			&self.deadline,
			&self.seals,
		)
	}

	/// The participant Nodes, in order.
	pub fn recipients(&self) -> Vec<String> {
		self.seals.iter().map(|seal| seal.node_id.clone()).collect()
	}

	/// Whether the participant at `node` claims `claim`.
	pub fn claims(&self, node: &str, claim: &str) -> bool {
		self.seals
			.iter()
			.find(|seal| seal.node_id == node)
			.is_some_and(|seal| seal.claims.iter().any(|own| own == claim))
	}

	pub fn validate(&self) -> Result<()> {
		if self.id.is_nil() {
			return Err(Error::Invalid(
				"transaction requires an ID and 1..16 participants".into(),
			));
		}
		validate_nodes(
			&self.coordinator,
			&self
				.seals
				.iter()
				.map(|seal| seal.node_id.as_str())
				.collect::<Vec<_>>(),
		)?;
		if !is_hex(&self.part.nonce)
			|| self
				.seals
				.iter()
				.any(|seal| !is_hex(&seal.hash) || seal.claims.iter().any(|claim| !is_hex(claim)))
		{
			return Err(Error::Invalid(
				"transaction seals must be SHA-256 hex".into(),
			));
		}
		validate_mutations(&self.part.mutations)?;
		// The owner vouches for its own seal; every other seal is vouched for by
		// its owner under the same digest.
		let own = self
			.seals
			.iter()
			.find(|seal| seal.node_id == self.part.node_id)
			.ok_or_else(|| {
				Error::Invalid("the part is not one of the sealed participants".into())
			})?;
		if *own != self.part.seal()? {
			return Err(Error::Conflict("the part does not match its seal".into()));
		}
		if serde_json::to_vec(self)?.len() > super::MAX_JSON_BYTES {
			return Err(Error::Invalid("a share must stay within 512 KiB".into()));
		}
		Ok(())
	}
}

impl Manifest {
	/// The random value that seals `node`'s part, derived from the coordinator's
	/// per-transaction secret `seed`.
	pub fn nonce(seed: &str, node: &str) -> String {
		sha256(format!("{seed}\0{node}"))
	}

	fn part(&self, seed: &str, node: &str) -> Result<Part> {
		let participant = self
			.local(node)
			.map_err(|_| Error::Invalid("the Node takes no part in this transaction".into()))?;
		Ok(Part {
			node_id: participant.node_id.clone(),
			nonce: Self::nonce(seed, node),
			mutations: participant.mutations.clone(),
		})
	}

	/// Every participant's seal, in participant order.
	pub fn seals(&self, seed: &str) -> Result<Vec<Seal>> {
		self.participants
			.iter()
			.map(|participant| self.part(seed, &participant.node_id)?.seal())
			.collect()
	}

	/// The transaction digest for this manifest sealed with `seed`.
	pub fn sealed_digest(&self, seed: &str) -> Result<String> {
		digest(
			&self.id,
			&self.coordinator,
			&self.isolation,
			&self.deadline,
			&self.seals(seed)?,
		)
	}

	/// What the participant at `node` receives.
	pub fn share(&self, seed: &str, node: &str) -> Result<Share> {
		Ok(Share {
			id: self.id,
			coordinator: self.coordinator.clone(),
			isolation: self.isolation.clone(),
			deadline: self.deadline,
			seals: self.seals(seed)?,
			part: self.part(seed, node)?,
		})
	}
}

#[cfg(test)]
mod tests;
