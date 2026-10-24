//! Disposable semantic graph edges use only fully authorized current source generations.
use super::access::Lease;
use crate::{Error, Result, database::native};
use aidash_domain::memory::{Policy, Unit, graph::Edge};
use aidash_domain::semantic::EmbeddingConfig;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use std::collections::BTreeMap;

/// Lock and validate the workspace's semantic index against the policy's embedding role.
async fn index(
	lease: &mut Lease<'_>,
	bank: &aidash_domain::memory::Bank,
	embedding: &EmbeddingConfig,
) -> Result<crate::apps::knowledge::serializers::contracts::Index> {
	let index =
		crate::semantic::models::SemanticIndexe::locked(&mut **lease.tx(), bank.workspace, false)
			.await?
			.ok_or(Error::SemanticUnavailable)?;
	let spec = index.configuration()?;
	if !spec.enabled || serde_json::to_value(&spec.embedding)? != serde_json::to_value(embedding)? {
		return Err(Error::SemanticUnavailable);
	}
	Ok(index)
}

/// The vector of this exact Unit revision and content in the current index generation,
/// or `None` when the generation has not (yet) indexed it.
async fn vector(
	lease: &mut Lease<'_>,
	index: &crate::apps::knowledge::serializers::contracts::Index,
	unit: &Unit,
	embedding: &EmbeddingConfig,
) -> Result<Option<Vec<f32>>> {
	let Some(entry) = native::query(
		&Query::select()
			.columns(["point_id", "state", "metadata", "index_revision"].map(Alias::new))
			.from(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").eq(Expr::value(unit.id)))
			.and_where(Expr::col("deleted").eq(false))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	else {
		return Ok(None);
	};
	let metadata: serde_json::Value = entry.try_get("metadata")?;
	if entry.try_get::<String>("state")? != "READY"
		|| entry.try_get::<i64>("index_revision")? != index.revision
		|| metadata["unit_revision"].as_i64() != Some(unit.revision)
	{
		return Ok(None);
	}
	let point: uuid::Uuid = entry.try_get("point_id")?;
	let digest: Option<String> = native::query_scalar(
		&Query::select()
			.column(Alias::new("content_digest"))
			.from(Alias::new("semantic_points"))
			.and_where(Expr::col("id").eq(Expr::value(point)))
			.and_where(Expr::col("retired").eq(false))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **lease.tx())
	.await?
	.flatten();
	if digest.as_deref()
		!= Some(aidash_domain::semantic::indexing::content_digest(&unit.content.text).as_str())
	{
		return Ok(None);
	}
	let vector: Option<String> = native::query_scalar(
		&Query::select()
			.expr(Expr::col("embedding").cast_as("text"))
			.from(Alias::new("semantic_vectors"))
			.and_where(Expr::col("id").eq(Expr::value(point)))
			.and_where(Expr::col("collection").eq(index.collection.as_str()))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **lease.tx())
	.await?;
	let Some(vector) = vector else {
		return Ok(None);
	};
	let values: Vec<f32> = serde_json::from_str(&vector)?;
	Ok((values.len() == embedding.dimensions).then_some(values))
}

/// Vectors for diversification. Unlike the semantic graph, a Unit the current index
/// generation has not embedded is omitted rather than failing recall.
pub(crate) async fn current_vectors(
	lease: &mut Lease<'_>,
	units: &[Unit],
	embedding: &EmbeddingConfig,
) -> Result<BTreeMap<uuid::Uuid, Vec<f32>>> {
	let Some(first) = units.first() else {
		return Ok(BTreeMap::new());
	};
	if units.iter().any(|unit| unit.bank != first.bank) {
		return Err(Error::Forbidden);
	}
	let index = index(lease, &first.bank, embedding).await?;
	let mut vectors = BTreeMap::new();
	for unit in units {
		if let Some(values) = vector(lease, &index, unit, embedding).await? {
			vectors.insert(unit.id, values);
		}
	}
	Ok(vectors)
}

pub(crate) async fn current(
	lease: &mut Lease<'_>,
	units: &[Unit],
	policy: &Policy,
	embedding: &EmbeddingConfig,
) -> Result<Vec<Edge>> {
	if units.len() < 2 {
		return Ok(vec![]);
	}
	let bank = &units[0].bank;
	if units.iter().any(|unit| &unit.bank != bank) {
		return Err(Error::Forbidden);
	}
	let index = index(lease, bank, embedding).await?;
	aidash_domain::memory::graph::validate_capacity(&policy.bounds, embedding.dimensions)?;
	let mut vectors = BTreeMap::new();
	for unit in units {
		let values = vector(lease, &index, unit, embedding)
			.await?
			.ok_or(Error::SemanticUnavailable)?;
		vectors.insert(unit.id, values);
	}
	aidash_domain::memory::graph::semantic(
		units,
		&vectors,
		policy.semantic_link_min_similarity_millionths,
		&policy.bounds,
	)
	.map_err(Into::into)
}
