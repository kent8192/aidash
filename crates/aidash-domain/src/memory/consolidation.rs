//! Bounded bottom-up observation groups over admitted facts and experiences.
//! Exact duplicates preserve case; related and conflicting facts retain every proof.
use super::{Bounds, Evidence, Unit};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

fn text(unit: &Unit) -> String {
	unit.content
		.text
		.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ")
}

/// A semantic verdict adds supplied support; it never drops mandatory sources,
/// changes their revisions, or crosses a bank, fact kind or learning category.
pub fn select(
	mandatory: &[Unit],
	candidates: &[Unit],
	selected: &[Evidence],
	bounds: &Bounds,
) -> Result<Vec<Unit>> {
	let Some(seed) = mandatory.first() else {
		return Err(Error::Invalid("empty mandatory observation support".into()));
	};
	let known: BTreeMap<_, _> = mandatory
		.iter()
		.chain(candidates)
		.map(|unit| (unit.evidence(), unit))
		.collect();
	let selection_count = selected.len();
	let selected: BTreeSet<_> = selected.iter().cloned().collect();
	if selected.len() != selection_count
		|| selected.len()
			> bounds
				.max_candidates
				.min(bounds.max_evidence)
				.min(bounds.max_graph_visits)
		|| known
			.values()
			.map(|unit| unit.id)
			.collect::<BTreeSet<_>>()
			.len() != known.len()
		|| mandatory
			.iter()
			.any(|unit| !selected.contains(&unit.evidence()))
		|| known.len() != mandatory.len() + candidates.len()
		|| selected.iter().any(|proof| !known.contains_key(proof))
		|| known.values().any(|unit| {
			!unit.visible()
				|| unit.bank != seed.bank
				|| unit.content.kind != seed.content.kind
				|| unit.content.kind.derived()
				|| unit.content.learning != seed.content.learning
		}) {
		return Err(Error::Invalid(
			"semantic consolidation changed complete support or scope".into(),
		));
	}
	let mut result: Vec<_> = selected
		.iter()
		.map(|proof| (*known[proof]).clone())
		.collect();
	result.sort_by_key(|unit| (unit.learned_at, unit.id));
	Ok(result)
}

/// Extending a live observation retains its complete admitted support. A new
/// semantic decision cannot quietly discard an earlier conflict in that group.
pub fn preserve_observations(
	selected: &[Unit],
	snapshot: &[Unit],
	bounds: &Bounds,
) -> Result<Vec<Unit>> {
	let Some(seed) = selected.first() else {
		return Err(Error::Invalid("empty observation support".into()));
	};
	let known: BTreeMap<_, _> = snapshot
		.iter()
		.filter(|unit| unit.visible() && !unit.content.kind.derived())
		.map(|unit| (unit.evidence(), unit))
		.collect();
	let mut support: BTreeSet<_> = selected.iter().map(Unit::evidence).collect();
	loop {
		let previous = support.len();
		for observation in snapshot.iter().filter(|unit| {
			unit.visible()
				&& unit.bank == seed.bank
				&& unit.content.kind == super::Kind::Observation
				// A manually composed observation may mix source categories or
				// primary evidence. It is not an automatic same-category group.
				&& !unit.content.evidence.is_empty()
				&& unit.content.evidence.iter().all(|proof| match proof {
					Evidence::Unit { bank, id, .. } if bank == &seed.bank => snapshot
						.iter().find(|source| source.id == *id)
						.is_none_or(|source| source.content.kind == seed.content.kind
							&& source.content.learning == seed.content.learning),
					_ => false,
				})
		}) {
			if observation
				.content
				.evidence
				.iter()
				.any(|proof| support.contains(proof))
			{
				support.extend(observation.content.evidence.iter().cloned());
			}
		}
		if support.len()
			> bounds
				.max_candidates
				.min(bounds.max_evidence)
				.min(bounds.max_graph_visits)
		{
			return Err(Error::Invalid(
				"complete semantic observation group exceeds its allowance".into(),
			));
		}
		if previous == support.len() {
			break;
		}
	}
	let mut result = Vec::with_capacity(support.len());
	for proof in support {
		let source = known
			.get(&proof)
			.ok_or_else(|| Error::Conflict("observation support changed".into()))?;
		if source.bank != seed.bank
			|| source.content.kind != seed.content.kind
			|| source.content.learning != seed.content.learning
		{
			return Err(Error::Invalid(
				"observation group crossed its source category".into(),
			));
		}
		result.push((**source).clone());
	}
	result.sort_by_key(|unit| (unit.learned_at, unit.id));
	Ok(result)
}

/// A connected entity/duplicate group has a stable earliest admitted seed. It
/// never mixes banks, world facts with experiences, or learning categories.
/// An oversized group fails instead of silently discarding conflicting evidence.
pub fn sources(trigger: &Unit, snapshot: &[Unit], bounds: &Bounds) -> Result<Vec<Unit>> {
	if !trigger.visible()
		|| trigger.content.kind.derived()
		|| snapshot.len() > bounds.max_units
		|| snapshot.iter().any(|unit| unit.bank != trigger.bank)
	{
		return Err(Error::Invalid("invalid observation snapshot".into()));
	}
	let units: BTreeMap<_, _> = snapshot.iter().map(|unit| (unit.id, unit)).collect();
	if units.len() != snapshot.len() || units.get(&trigger.id).is_none_or(|unit| **unit != *trigger)
	{
		return Err(Error::Invalid("observation trigger changed".into()));
	}
	let mut queue = VecDeque::from([trigger.id]);
	let mut seen = BTreeSet::new();
	while let Some(id) = queue.pop_front() {
		if !seen.insert(id) {
			continue;
		}
		if seen.len()
			> bounds
				.max_candidates
				.min(bounds.max_evidence)
				.min(bounds.max_graph_visits)
		{
			return Err(Error::Invalid(
				"complete observation group exceeds its allowance".into(),
			));
		}
		let current = units[&id];
		let entities: BTreeSet<_> = current
			.content
			.entities
			.iter()
			.flat_map(|entity| entity.keys())
			.collect();
		let normalized = text(current);
		for unit in snapshot {
			if !seen.contains(&unit.id)
				&& unit.visible()
				&& unit.content.kind == trigger.content.kind
				&& unit.content.learning == trigger.content.learning
				&& (text(unit) == normalized
					|| unit
						.content
						.entities
						.iter()
						.any(|entity| !entities.is_disjoint(&entity.keys())))
				&& !queue.contains(&unit.id)
			{
				queue.push_back(unit.id);
			}
		}
	}
	let mut result: Vec<_> = seen.into_iter().map(|id| units[&id].clone()).collect();
	result.sort_by_key(|unit| (unit.learned_at, unit.id));
	Ok(result)
}
