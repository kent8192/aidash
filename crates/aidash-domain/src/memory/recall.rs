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

/// Symmetric structural redundancy: a derived Unit and the candidate Units it cites.
pub type Related = BTreeMap<Uuid, BTreeSet<Uuid>>;

/// Derived memory (Observation, mental model) is redundant with the exact current
/// revisions it cites. Citations of Units outside `units` are ignored.
pub fn related(units: &[Unit]) -> Related {
	let by_id: BTreeMap<_, _> = units.iter().map(|unit| (unit.id, unit)).collect();
	let mut related = Related::new();
	for derived in units.iter().filter(|unit| unit.content.kind.derived()) {
		for evidence in &derived.content.evidence {
			let super::Evidence::Unit { bank, id, revision } = evidence else {
				continue;
			};
			if *id != derived.id
				&& *bank == derived.bank
				&& by_id
					.get(id)
					.is_some_and(|source| source.revision == *revision && source.bank == *bank)
			{
				related.entry(derived.id).or_default().insert(*id);
				related.entry(*id).or_default().insert(derived.id);
			}
		}
	}
	related
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diversified {
	pub selected: Vec<Ranked>,
	/// Candidates without a current vector. They compete on relevance alone and
	/// neither penalize nor are penalized by embedding similarity.
	pub skipped: Vec<Uuid>,
}

/// Maximal Marginal Relevance over an already authorized, reranked candidate list.
///
/// Each step picks `argmax λ·relevance − (1−λ)·max_sim(selected)`, where relevance is
/// the input score scaled to [0, 1] and `max_sim` is the largest non-negative cosine
/// to a selected candidate. A candidate structurally `related` to a selected one (a
/// derived summary and its cited sources) counts as maximally similar (1.0), so λ still
/// decides whether a highly relevant source may follow its summary. Exact ties follow
/// `compare`: retention when `retention` is given, then the stable Unit ID. This only
/// reorders or drops the given candidates; it never introduces an ID.
pub fn diversify(
	ranked: &[Ranked],
	vectors: &BTreeMap<Uuid, Vec<f32>>,
	related: &Related,
	retention: Option<&BTreeMap<Uuid, f64>>,
	lambda_millionths: u32,
	limit: usize,
) -> Result<Diversified> {
	if !(1..=1_000_000).contains(&lambda_millionths) {
		return Err(Error::Invalid(
			"diversity lambda must be within 1..=1_000_000 millionths".into(),
		));
	}
	let ids: BTreeSet<_> = ranked.iter().map(|hit| hit.id).collect();
	if ids.len() != ranked.len() || ranked.iter().any(|hit| !hit.score.is_finite()) {
		return Err(Error::Invalid(
			"diversification needs unique candidates with finite scores".into(),
		));
	}
	let lambda = f64::from(lambda_millionths) / 1_000_000.;
	let usable: Vec<_> = ranked.iter().map(|hit| vectors.get(&hit.id)).collect();
	let dimension = usable.iter().flatten().map(|values| values.len()).next();
	if usable.iter().flatten().any(|values| {
		Some(values.len()) != dimension
			|| values.is_empty()
			|| values.iter().any(|value| !value.is_finite())
			|| values.iter().all(|value| *value == 0.)
	}) {
		return Err(Error::Invalid("invalid diversification embeddings".into()));
	}
	let norms: Vec<_> = usable
		.iter()
		.map(|values| values.map(|values| super::graph::norm(values)))
		.collect();
	let mut skipped: Vec<_> = ranked
		.iter()
		.zip(&usable)
		.filter(|(_, values)| values.is_none())
		.map(|(hit, _)| hit.id)
		.collect();
	skipped.sort();
	// Non-negative scores keep their ratio to the top score; a negative score shifts the
	// scale so the minimum maps to zero. Halving keeps the span finite for any finite scores.
	let (base, high) = ranked
		.iter()
		.fold((0.0_f64, f64::NEG_INFINITY), |(base, high), hit| {
			(base.min(hit.score), high.max(hit.score))
		});
	let span = high / 2. - base / 2.;
	let relevance: Vec<_> = ranked
		.iter()
		.map(|hit| {
			if span > 0. {
				(hit.score / 2. - base / 2.) / span
			} else {
				1.
			}
		})
		.collect();
	let mut remaining: Vec<usize> = (0..ranked.len()).collect();
	let mut max_sim = vec![0.0_f64; ranked.len()];
	let mut selected = Vec::new();
	while selected.len() < limit && !remaining.is_empty() {
		let best = remaining
			.iter()
			.copied()
			.max_by(|a, b| {
				let value =
					|index: usize| lambda * relevance[index] - (1. - lambda) * max_sim[index];
				// Exact ties follow the baseline order: retention (when decay is on), then Unit ID.
				value(*a).total_cmp(&value(*b)).then_with(|| {
					compare(ranked[*a].id, 0., ranked[*b].id, 0., retention).reverse()
				})
			})
			.expect("remaining is not empty");
		remaining.retain(|index| *index != best);
		let chosen = &ranked[best];
		for index in remaining.iter().copied() {
			if related
				.get(&chosen.id)
				.is_some_and(|peers| peers.contains(&ranked[index].id))
			{
				max_sim[index] = 1.;
			}
			if let (Some(left), Some(right), Some(left_norm), Some(right_norm)) =
				(usable[best], usable[index], norms[best], norms[index])
			{
				max_sim[index] =
					max_sim[index].max(super::graph::cosine(left, right, left_norm, right_norm));
			}
		}
		selected.push(chosen.clone());
	}
	Ok(Diversified { selected, skipped })
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
