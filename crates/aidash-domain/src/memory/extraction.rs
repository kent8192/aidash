//! Batch-local causal references are resolved only to host-issued admitted identities.
use super::{Bounds, Content, Link, LinkKind};
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
	Retain,
	RunCandidate,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CausalRelation {
	pub cause: usize,
	pub effect: usize,
	pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Extraction {
	pub facts: Vec<Content>,
	pub causal: Vec<CausalRelation>,
}
impl Extraction {
	pub fn resolve(mut self, ids: &[Uuid], bounds: &Bounds) -> Result<Vec<Content>> {
		if ids.len() != self.facts.len()
			|| ids.len() > bounds.max_candidates
			|| ids.iter().any(Uuid::is_nil)
			|| ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
			|| self.causal.len()
				> bounds
					.max_candidates
					.checked_mul(bounds.max_links)
					.ok_or_else(|| Error::Invalid("causal link bound overflow".into()))?
		{
			return Err(Error::Invalid(
				"invalid extraction batch identities or size".into(),
			));
		}
		let mut seen = BTreeSet::new();
		for relation in self.causal {
			if relation.cause >= ids.len()
				|| relation.effect >= ids.len()
				|| relation.cause == relation.effect
				|| !relation.weight.is_finite()
				|| relation.weight <= 0.
				|| relation.weight > 1.
				|| !seen.insert((relation.cause, relation.effect))
			{
				return Err(Error::Invalid(
					"extractor invented a causal target or repeated an edge".into(),
				));
			}
			self.facts[relation.effect].links.push(Link {
				target: ids[relation.cause],
				revision: 1,
				kind: LinkKind::CausedBy,
				weight: relation.weight,
			});
			self.facts[relation.cause].links.push(Link {
				target: ids[relation.effect],
				revision: 1,
				kind: LinkKind::Causes,
				weight: relation.weight,
			});
		}
		for fact in &self.facts {
			fact.validate(bounds)?;
		}
		Ok(self.facts)
	}
}
