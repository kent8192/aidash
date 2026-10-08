//! Live authority and source revisions are held through all four recall arms and delivery.
use super::{access::Lease, candidates, native_memory as repository, units};
use crate::semantic::services::{memory_models::Models, native_memory as service};
use crate::{Error, Result, database::native, store::Store};
use aidash_application::ports::{VectorIndex, memory::*};
use aidash_domain::{
	memory::*,
	registry::EntityRef,
	semantic::{EmbeddingConfig, VectorFilter},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct Scope<'a, 'scope> {
	pub store: &'a Store,
	pub lease: &'a mut Lease<'scope>,
	pub models: &'a Models,
	pub delivered: &'a mut Vec<Unit>,
}
impl Scope<'_, '_> {
	async fn stamp(&mut self, bank: &Bank) -> Result<String> {
		let bank_id = repository::bank_id(self.lease, bank, false)
			.await?
			.ok_or(Error::Forbidden)?;
		let revision: i64 = native::query_scalar(
			&Query::select()
				.column(Alias::new("revision"))
				.from(Alias::new("memory_banks"))
				.and_where(Expr::col("id").eq(Expr::value(bank_id)))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **self.lease.tx())
		.await?;
		let authority: Option<i64> = native::query_scalar(
			&Query::select()
				.column(Alias::new("revision"))
				.from(Alias::new("authorization_bundles"))
				.and_where(Expr::col("tenant").eq(bank.tenant.as_str()))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.lease.tx())
		.await?;
		Ok(serde_json::to_string(&(
			revision,
			authority,
			self.lease.saved()?,
		))?)
	}
	async fn search(
		&mut self,
		bank: &Bank,
		model: &EntityRef,
		text: &str,
		allowed: &[Uuid],
		limit: usize,
		allowance: Allowance,
	) -> Result<Produced<Vec<Uuid>>> {
		if model != &self.models.policy.embedding {
			return Err(Error::Conflict("memory embedding role changed".into()));
		}
		let index = crate::semantic::models::SemanticIndexe::locked(
			&mut **self.lease.tx(),
			bank.workspace,
			false,
		)
		.await?
		.ok_or(Error::SemanticUnavailable)?;
		let spec = index.configuration()?;
		let config: EmbeddingConfig =
			serde_json::from_value(self.models.role(model, "embedding")?.config.clone())?;
		if !spec.enabled || serde_json::to_value(&spec.embedding)? != serde_json::to_value(&config)?
		{
			return Err(Error::SemanticUnavailable);
		}
		let mut points = Vec::with_capacity(allowed.len());
		let mut point_to_unit = std::collections::BTreeMap::new();
		for id in allowed {
			let unit = units::load(self.lease, *id, false)
				.await?
				.ok_or(Error::SemanticUnavailable)?;
			if unit.bank != *bank || !unit.visible() {
				return Err(Error::Conflict("memory search scope changed".into()));
			}
			let row = native::query(
				&Query::select()
					.columns(["point_id", "index_revision", "state", "metadata"].map(Alias::new))
					.from(Alias::new("semantic_entries"))
					.and_where(Expr::col("id").eq(Expr::value(*id)))
					.and_where(Expr::col("deleted").eq(false))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.lease.tx())
			.await?
			.ok_or(Error::SemanticUnavailable)?;
			let metadata: Value = row.try_get("metadata")?;
			if row.try_get::<String>("state")? != "READY"
				|| row.try_get::<i64>("index_revision")? != index.revision
				|| metadata["unit_revision"].as_i64() != Some(unit.revision)
			{
				return Err(Error::SemanticUnavailable);
			}
			let point: Uuid = row.try_get("point_id")?;
			let digest: Option<String> = native::query_scalar(
				&Query::select()
					.column(Alias::new("content_digest"))
					.from(Alias::new("semantic_points"))
					.and_where(Expr::col("id").eq(Expr::value(point)))
					.and_where(Expr::col("retired").eq(false))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **self.lease.tx())
			.await?
			.flatten();
			if digest.as_deref()
				!= Some(&aidash_domain::semantic::indexing::content_digest(
					&unit.content.text,
				)) {
				return Err(Error::SemanticUnavailable);
			}
			points.push(point);
			point_to_unit.insert(point, *id);
		}
		let transport = crate::bootstrap::semantic_transport(self.store);
		if !transport
			.present(&spec.vector, &index.collection, &points)
			.await?
		{
			return Err(Error::SemanticUnavailable);
		}
		let embedded = self.models.embed(model, text, allowance).await?;
		let results = transport
			.query(
				&spec.vector,
				&index.collection,
				&embedded.output,
				VectorFilter {
					workspace: bank.workspace,
					tenant: &bank.tenant,
					allowed: &points,
				},
				limit,
			)
			.await?;
		let mut ids = vec![];
		for point in results {
			let id = point_to_unit
				.get(&point.id)
				.ok_or(Error::SemanticUnavailable)?;
			if point.payload["entry_id"].as_str() != Some(id.to_string().as_str())
				|| point.payload["index_revision"].as_i64() != Some(index.revision)
			{
				return Err(Error::SemanticUnavailable);
			}
			ids.push(*id);
		}
		Ok(Produced {
			output: ids,
			usage: embedded.usage,
		})
	}
}
#[async_trait]
impl MemoryScope for Scope<'_, '_> {
	async fn authorize(
		&mut self,
		bank: &Bank,
		provider: &EntityRef,
		action: &str,
	) -> aidash_application::Result<()> {
		if bank != &self.models.bank || provider != &self.models.provider {
			return Err(aidash_application::Error::Forbidden);
		}
		service::scope(self.store, self.lease, bank, action).await?;
		service::bank_provider(self.lease, bank, provider)
			.await
			.map_err(Into::into)
	}
	async fn snapshot(
		&mut self,
		bank: &Bank,
		limit: usize,
	) -> aidash_application::Result<Snapshot> {
		let units = repository::list(
			self.lease,
			bank,
			limit,
			self.models.policy.bounds.max_graph_visits,
		)
		.await?;
		let embedding: EmbeddingConfig = serde_json::from_value(
			self.models
				.role(&self.models.policy.embedding, "embedding")?
				.config
				.clone(),
		)?;
		let graph =
			super::memory_graph::current(self.lease, &units, &self.models.policy, &embedding)
				.await?;
		Ok(Snapshot {
			units,
			graph,
			authority_revision: self.stamp(bank).await?,
		})
	}
	async fn current(
		&mut self,
		bank: &Bank,
		evidence: &[Evidence],
	) -> aidash_application::Result<()> {
		units::current(
			self.lease,
			bank.workspace,
			evidence,
			self.models.policy.bounds.max_graph_visits,
		)
		.await
		.map_err(Into::into)
	}
	async fn mutate(
		&mut self,
		mutation: &Mutation,
		bounds: &Bounds,
	) -> aidash_application::Result<Vec<Unit>> {
		repository::mutate_origin(self.lease, mutation, bounds, self.models.run)
			.await
			.map_err(Into::into)
	}
	async fn propose(
		&mut self,
		bank: &Bank,
		run: &Evidence,
		content: &[Content],
		bounds: &Bounds,
	) -> aidash_application::Result<Vec<Candidate>> {
		candidates::propose(self.lease, bank, run, content, bounds)
			.await
			.map_err(Into::into)
	}
	async fn review(
		&mut self,
		id: Uuid,
		expected: i64,
		mutation: Option<&Mutation>,
		bounds: &Bounds,
	) -> aidash_application::Result<Option<Unit>> {
		candidates::review(
			self.lease,
			&self.models.bank,
			self.models.operation,
			id,
			expected,
			mutation,
			bounds,
		)
		.await
		.map_err(Into::into)
	}
	async fn publish(
		&mut self,
		source: &Evidence,
		mutation: &Mutation,
		bounds: &Bounds,
	) -> aidash_application::Result<Vec<Unit>> {
		super::publications::publish(self.lease, source, mutation, bounds)
			.await
			.map_err(Into::into)
	}
	async fn semantic(
		&mut self,
		bank: &Bank,
		model: &EntityRef,
		text: &str,
		allowed: &[Uuid],
		limit: usize,
		allowance: Allowance,
	) -> aidash_application::Result<Produced<Vec<Uuid>>> {
		self.search(bank, model, text, allowed, limit, allowance)
			.await
			.map_err(Into::into)
	}
	async fn keyword(
		&mut self,
		bank: &Bank,
		text: &str,
		allowed: &[Uuid],
		limit: usize,
	) -> aidash_application::Result<Vec<Uuid>> {
		repository::keyword(self.lease, bank, text, allowed, limit)
			.await
			.map_err(Into::into)
	}
	async fn deliver(
		&mut self,
		bank: &Bank,
		authority_revision: &str,
		evidence: &[Evidence],
	) -> aidash_application::Result<()> {
		service::scope(self.store, self.lease, bank, "memory.read").await?;
		if self.stamp(bank).await? != authority_revision {
			return Err(aidash_application::Error::Conflict(
				"memory authority snapshot changed".into(),
			));
		}
		let mut selected = Vec::with_capacity(evidence.len());
		for source in evidence {
			let Evidence::Unit {
				bank: origin,
				id,
				revision,
			} = source
			else {
				return Err(aidash_application::Error::Invalid(
					"memory delivery requires selected Unit roots".into(),
				));
			};
			let unit = units::load(self.lease, *id, false)
				.await?
				.ok_or(Error::Forbidden)?;
			if origin != bank || unit.bank != *bank || unit.revision != *revision || !unit.visible()
			{
				return Err(aidash_application::Error::Conflict(
					"memory source changed before delivery".into(),
				));
			}
			units::unexpired(self.lease, &unit).await?;
			units::current(
				self.lease,
				bank.workspace,
				&unit.content.evidence,
				self.models.policy.bounds.max_graph_visits,
			)
			.await?;
			selected.push(unit);
		}
		self.delivered.extend(selected);
		Ok(())
	}
}
