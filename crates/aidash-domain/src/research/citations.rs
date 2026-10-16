//! Source citations. A final answer cites Evidence fragments with
//! `[[cite:ev_<uuid>]]`; the host accepts only fragments delivered to the same
//! Run. Verification is selected by declared tool behavior, never a tool name.
use std::{fmt, str::FromStr};
use uuid::Uuid;

pub const MARKER_OPEN: &str = "[[cite:";
pub const MARKER_CLOSE: &str = "]]";
/// Distinct references verified per answer.
pub const MAX_CITATIONS: usize = 200;

/// Recorded once in a Run's history when an answer cites unknown evidence.
pub const CORRECTION_INSTRUCTION: &str = "Correct the answer once. Cite only evidence_ref values returned by research tools in this Run, formatted [[cite:ev_<uuid>]]. If evidence is unavailable, state that the claim could not be verified. Do not invent references.";
/// Published instead of an answer whose citations remain invalid after correction.
pub const WITHHELD_ANSWER: &str = "Research evidence could not be verified. The answer was withheld because its citations were unavailable or invalid.";

/// The identity of one Evidence fragment, rendered `ev_<uuid>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceRef(pub Uuid);

impl fmt::Display for EvidenceRef {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "ev_{}", self.0)
	}
}

impl FromStr for EvidenceRef {
	type Err = crate::Error;
	fn from_str(value: &str) -> crate::Result<Self> {
		value
			.strip_prefix("ev_")
			.filter(|id| id.len() == 36)
			.and_then(|id| Uuid::parse_str(id).ok())
			.map(Self)
			.ok_or_else(|| crate::Error::Invalid("invalid evidence reference".into()))
	}
}

impl serde::Serialize for EvidenceRef {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.collect_str(self)
	}
}

impl<'de> serde::Deserialize<'de> for EvidenceRef {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		<String as serde::Deserialize>::deserialize(deserializer)?
			.parse()
			.map_err(serde::de::Error::custom)
	}
}

/// The citations of one answer: well-formed references in first-use order and
/// any malformed or unterminated marker contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Citations {
	pub references: Vec<EvidenceRef>,
	pub malformed: Vec<String>,
}

impl Citations {
	pub fn is_empty(&self) -> bool {
		self.references.is_empty() && self.malformed.is_empty()
	}
}

pub fn citations(text: &str) -> Citations {
	let mut result = Citations::default();
	let mut remaining = text;
	while let Some((_, after)) = remaining.split_once(MARKER_OPEN) {
		let Some((reference, rest)) = after.split_once(MARKER_CLOSE) else {
			result.malformed.push(after.chars().take(64).collect());
			break;
		};
		match reference.parse::<EvidenceRef>() {
			Ok(reference) if !result.references.contains(&reference) => {
				result.references.push(reference);
			}
			Ok(_) => {}
			Err(_) => result.malformed.push(reference.chars().take(64).collect()),
		}
		if result.references.len() + result.malformed.len() > MAX_CITATIONS {
			result.malformed.push("citation limit exceeded".into());
			break;
		}
		remaining = rest;
	}
	result
}

/// The Harness's verdict on an answer's citations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
	/// Publish the answer unchanged.
	Accept,
	/// Ask the model once to correct the listed references.
	Correct(Vec<String>),
	/// Publish [`WITHHELD_ANSWER`] instead of the answer.
	Withhold,
}

/// `unknown` are well-formed references the Run's evidence store does not hold.
pub fn verdict(citations: &Citations, unknown: &[EvidenceRef], corrected: bool) -> Verdict {
	let invalid = citations
		.malformed
		.iter()
		.cloned()
		.chain(unknown.iter().map(ToString::to_string))
		.collect::<Vec<_>>();
	if invalid.is_empty() {
		Verdict::Accept
	} else if corrected {
		Verdict::Withhold
	} else {
		Verdict::Correct(invalid)
	}
}

#[cfg(test)]
mod tests;
