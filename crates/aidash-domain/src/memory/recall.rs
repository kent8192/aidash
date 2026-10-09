//! Bounded four-arm fusion translated from the pinned Hindsight retrieval contract.
use crate::{
	Error, Result,
	memory::{Bounds, LinkKind, TimeRange, Unit},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct Rankings {
	pub semantic: Vec<Uuid>,
	pub keyword: Vec<Uuid>,
	pub graph: Vec<Uuid>,
	pub temporal: Vec<Uuid>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
	pub id: Uuid,
	pub score: f64,
}

/// RRF k=60. Duplicate provider hits cannot amplify an arm; ties use stable unit IDs.
pub fn fuse(rankings: &Rankings, allowed: &BTreeSet<Uuid>, limit: usize) -> Result<Vec<Ranked>> {
	let mut scores = BTreeMap::<Uuid, f64>::new();
	for arm in [
		&rankings.semantic,
		&rankings.keyword,
		&rankings.graph,
		&rankings.temporal,
	] {
		let mut seen = BTreeSet::new();
		for id in arm {
			if !allowed.contains(id) {
				return Err(Error::Invalid(
					"retrieval returned an unauthorized candidate".into(),
				));
			}
			if seen.insert(*id) {
				let rank = seen.len();
				*scores.entry(*id).or_default() += 1.0 / (60.0 + rank as f64);
			}
		}
	}
	let mut ranked: Vec<_> = scores
		.into_iter()
		.map(|(id, score)| Ranked { id, score })
		.collect();
	ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
	ranked.truncate(limit);
	Ok(ranked)
}

/// Apply the prior to relevance hits before the candidate cap. Missing scores are identity.
pub fn fuse_with_prior(
	rankings: &Rankings,
	allowed: &BTreeSet<Uuid>,
	limit: usize,
	decay: Option<&super::Decay>,
	retention: &BTreeMap<Uuid, f64>,
) -> Result<Vec<Ranked>> {
	let Some(decay) = decay else {
		return fuse(rankings, allowed, limit);
	};
	let mut ranked = fuse(rankings, allowed, allowed.len())?;
	for hit in &mut ranked {
		let score = retention.get(&hit.id).copied().unwrap_or(1.0);
		if !score.is_finite() || score <= 0.0 || score > 1.0 {
			return Err(Error::Invalid("retention score must be in (0, 1]".into()));
		}
		hit.score *= super::decay::prior(decay, score);
	}
	ranked.sort_by(|a, b| compare(a.id, a.score, b.id, b.score, Some(retention)));
	ranked.truncate(limit);
	Ok(ranked)
}

pub fn compare(
	a: Uuid,
	a_score: f64,
	b: Uuid,
	b_score: f64,
	retention: Option<&BTreeMap<Uuid, f64>>,
) -> std::cmp::Ordering {
	b_score
		.total_cmp(&a_score)
		.then_with(|| {
			retention.map_or(std::cmp::Ordering::Equal, |scores| {
				scores
					.get(&b)
					.copied()
					.unwrap_or(1.0)
					.total_cmp(&scores.get(&a).copied().unwrap_or(1.0))
			})
		})
		.then(a.cmp(&b))
}

/// Traversal only sees the preauthorized snapshot. No link grants read permission.
pub fn graph(units: &[Unit], seeds: &[Uuid], bounds: &Bounds) -> Vec<Uuid> {
	graph_with_edges(units, seeds, &[], bounds)
}

pub fn graph_with_edges(
	units: &[Unit],
	seeds: &[Uuid],
	edges: &[super::graph::Edge],
	bounds: &Bounds,
) -> Vec<Uuid> {
	let allowed: BTreeMap<_, _> = units
		.iter()
		.filter(|u| u.visible())
		.map(|u| (u.id, u))
		.collect();
	let mut queue: VecDeque<_> = seeds
		.iter()
		.filter(|id| allowed.contains_key(id))
		.map(|id| (*id, 0usize))
		.collect();
	let mut seen = BTreeSet::new();
	let mut scores = BTreeMap::<Uuid, f64>::new();
	while let Some((id, depth)) = queue.pop_front() {
		if seen.len() >= bounds.max_graph_visits {
			break;
		}
		if !seen.insert(id) || depth >= bounds.max_graph_hops {
			continue;
		}
		let unit = allowed[&id];
		let mut neighbors = BTreeMap::<Uuid, f64>::new();
		for link in unit.content.links.iter().chain(
			edges
				.iter()
				.filter(|edge| edge.source == unit.id && edge.source_revision == unit.revision)
				.map(|edge| &edge.target),
		) {
			let Some(target) = allowed.get(&link.target) else {
				continue;
			};
			if target.revision != link.revision
				|| target.bank != unit.bank
				|| !link.weight.is_finite()
				|| link.weight <= 0.
			{
				continue;
			}
			let weight = match link.kind {
				LinkKind::Entity => (link.weight * 0.5).tanh(),
				_ => link.weight,
			};
			neighbors
				.entry(link.target)
				.and_modify(|old| *old = old.max(weight))
				.or_insert(weight);
		}
		// Resolve implicit entity/occurrence edges from this exact authorized
		// snapshot. A changed alias/window is visible immediately; no stale
		// materialized graph generation can grant access to another bank.
		let keys: BTreeSet<_> = unit
			.content
			.entities
			.iter()
			.flat_map(|entity| entity.keys())
			.collect();
		for target in allowed.values() {
			if target.id == id || target.bank != unit.bank {
				continue;
			}
			let other: BTreeSet<_> = target
				.content
				.entities
				.iter()
				.flat_map(|entity| entity.keys())
				.collect();
			let shared = keys.intersection(&other).count();
			let temporal = unit
				.content
				.occurred
				.as_ref()
				.zip(target.content.occurred.as_ref())
				.is_some_and(|(left, right)| left.start <= right.end && right.start <= left.end);
			if shared > 0 || temporal {
				let entity = if shared > 0 {
					(shared as f64 / keys.union(&other).count() as f64 * 0.5).tanh()
				} else {
					0.0
				};
				let weight = entity.max(if temporal { 0.5 } else { 0.0 });
				neighbors
					.entry(target.id)
					.and_modify(|old| *old = old.max(weight))
					.or_insert(weight);
			}
		}
		let mut neighbors: Vec<_> = neighbors.into_iter().collect();
		neighbors.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
		for (target, weight) in neighbors.into_iter().take(bounds.max_links) {
			*scores.entry(target).or_default() += weight / (depth + 1) as f64;
			if queue.len() < bounds.max_graph_visits {
				queue.push_back((target, depth + 1));
			}
		}
	}
	let mut ranked: Vec<_> = scores.into_iter().collect();
	ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
	ranked
		.into_iter()
		.take(bounds.max_candidates)
		.map(|(id, _)| id)
		.collect()
}

/// Overlapping occurrence windows are preferred; learned time is not occurrence time.
pub fn temporal(units: &[Unit], window: Option<&TimeRange>, limit: usize) -> Vec<Uuid> {
	let Some(window) = window else {
		return vec![];
	};
	let mut ranked: Vec<_> = units
		.iter()
		.filter(|u| u.visible())
		.filter_map(|unit| {
			let time = unit.content.occurred.as_ref()?;
			if time.end < window.start || time.start > window.end {
				return None;
			}
			let center = window.start.timestamp() as i128 + window.end.timestamp() as i128;
			let distance =
				(time.start.timestamp() as i128 + time.end.timestamp() as i128 - center).abs();
			Some((unit.id, distance))
		})
		.collect();
	ranked.sort_by_key(|(id, distance)| (*distance, *id));
	ranked.into_iter().take(limit).map(|(id, _)| id).collect()
}
