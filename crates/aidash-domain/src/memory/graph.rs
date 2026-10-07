//! Exact-revision semantic edges from the current authorized embedding generation.
//! Adapted from Hindsight retain/link_creation.py; edges confer no source authority.
use super::{Bounds, Link, LinkKind, Unit};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
	pub source: Uuid,
	pub source_revision: i64,
	pub target: Link,
}

// Admission budgets the worst-case dot products independently of graph output caps.
const MAX_SIMILARITY_SCALARS: usize = 32 * 1024 * 1024;

pub fn validate_capacity(bounds: &Bounds, dimensions: usize) -> Result<()> {
	if dimensions == 0
		|| bounds
			.max_units
			.checked_mul(bounds.max_units)
			.and_then(|pairs| pairs.checked_mul(dimensions))
			.is_none_or(|scalars| scalars > MAX_SIMILARITY_SCALARS)
	{
		return Err(Error::Invalid(
			"memory semantic graph exceeds its compute allowance".into(),
		));
	}
	Ok(())
}

pub fn semantic(
	units: &[Unit],
	vectors: &BTreeMap<Uuid, Vec<f32>>,
	min_similarity_millionths: u32,
	bounds: &Bounds,
) -> Result<Vec<Edge>> {
	if units.len() > bounds.max_units
		|| !(1..=1_000_000).contains(&min_similarity_millionths)
		|| units
			.iter()
			.map(|unit| unit.id)
			.collect::<BTreeSet<_>>()
			.len() != units.len()
		|| vectors.len() != units.len()
		|| units
			.iter()
			.any(|unit| !unit.visible() || !vectors.contains_key(&unit.id))
	{
		return Err(Error::Invalid("invalid semantic graph snapshot".into()));
	}
	let dimension = vectors.values().next().map_or(0, Vec::len);
	if !units.is_empty()
		&& (dimension == 0
			|| dimension > 8192
			|| vectors.values().any(|values| {
				values.len() != dimension
					|| values.iter().any(|value| !value.is_finite())
					|| values.iter().all(|value| *value == 0.)
			})) {
		return Err(Error::Invalid("invalid semantic graph embeddings".into()));
	}
	validate_capacity(bounds, dimension.max(1))?;
	let norms: BTreeMap<_, _> = vectors
		.iter()
		.map(|(id, values)| {
			(
				*id,
				values
					.iter()
					.map(|value| f64::from(*value).powi(2))
					.sum::<f64>()
					.sqrt(),
			)
		})
		.collect();
	let minimum = f64::from(min_similarity_millionths) / 1_000_000.;
	let mut result = vec![];
	for source in units {
		let mut neighbors = vec![];
		for target in units {
			if source.id == target.id || source.bank != target.bank {
				continue;
			}
			let left = &vectors[&source.id];
			let right = &vectors[&target.id];
			let dot: f64 = left
				.iter()
				.zip(right)
				.map(|(a, b)| f64::from(*a) * f64::from(*b))
				.sum();
			let similarity = (dot / (norms[&source.id] * norms[&target.id])).clamp(-1., 1.);
			// Identical finite vectors can round just below one after normalization.
			if similarity >= minimum || (minimum == 1. && similarity >= 1. - 1e-12) {
				neighbors.push(Link {
					target: target.id,
					revision: target.revision,
					kind: LinkKind::Semantic,
					weight: similarity,
				});
			}
		}
		neighbors.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(a.target.cmp(&b.target)));
		for target in neighbors.into_iter().take(bounds.max_links) {
			result.push(Edge {
				source: source.id,
				source_revision: source.revision,
				target,
			});
		}
	}
	Ok(result)
}
