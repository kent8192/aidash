//! Durable source lineage retains every origin's live authority and ancestor budget.
use super::{Models, definition};
use crate::apps::knowledge::repositories::{access::Lease, unit_origins};
use crate::{Error, Result, authorization::access::Access};
use aidash_domain::{memory::Unit, registry::EntityRef, semantic::EmbeddingConfig};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(super) struct OriginLease {
	pub run: Option<Uuid>,
	pub access: Access,
}
pub(super) struct Authorities {
	pub runs: BTreeSet<Uuid>,
	pub leases: Vec<OriginLease>,
}

impl Models {
	pub(super) async fn add_source_origins<'a>(
		&self,
		units: impl Iterator<Item = &'a Unit> + Send,
	) -> Result<()> {
		let mut lease = Lease::begin(
			&self.store,
			&crate::authorization::identity::Actor::Operator,
		)
		.await?;
		let result = async {
			let mut runs = BTreeSet::new();
			for unit in units {
				let origin = unit_origins::load(&mut lease, unit.id)
					.await?
					.ok_or(Error::Forbidden)?;
				if origin.revision != unit.revision {
					return Err(Error::Forbidden);
				}
				runs.extend(origin.runs);
				if runs.len() > self.policy.bounds.max_graph_visits {
					return Err(Error::Conflict(
						"memory origin lineage exceeds its allowance".into(),
					));
				}
			}
			Ok(runs)
		}
		.await;
		let runs = lease.finish(result).await?;
		self.add_origins(runs).await
	}
	pub(super) async fn add_origins(&self, runs: BTreeSet<Uuid>) -> Result<()> {
		let mut authorities = self.inference.lock().await;
		if authorities.runs.union(&runs).count() > self.policy.bounds.max_graph_visits {
			return Err(Error::Conflict(
				"memory origin lineage exceeds its allowance".into(),
			));
		}
		for run in runs {
			if authorities.runs.contains(&run) {
				continue;
			}
			if let Some(access) = self.current_origin(run).await? {
				authorities.leases.push(OriginLease {
					run: Some(run),
					access,
				});
			}
			authorities.runs.insert(run);
		}
		Ok(())
	}
	async fn current_origin(&self, run: Uuid) -> Result<Option<Access>> {
		let record: aidash_domain::Run = crate::database::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("runs"))
				.and_where(Expr::col("id").eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&self.store.control_pool)
		.await?;
		if record.home_node != self.bank.home || record.workspace_id != self.bank.workspace {
			return Err(Error::Forbidden);
		}
		if let Some(mut parent) =
			crate::authorization::execution::access_for_run(&self.store, &record.metadata(), true)
				.await?
		{
			let result = async {
				let mut dependencies = Vec::with_capacity(self.roles.len() + 1);
				for role in &self.roles {
					dependencies.push(
						definition(
							&mut Lease::Inherited(&mut parent),
							&EntityRef {
								id: role.id.clone(),
								version: role.version.clone(),
							},
							&role.kind,
						)
						.await?,
					);
				}
				dependencies.push(
					definition(&mut Lease::Inherited(&mut parent), &self.provider, "memory")
						.await?,
				);
				parent.track_registry(&dependencies).await?;
				let mut child = Access::under_lease_on(&parent, &self.store.control_pool).await?;
				child.suspend().await?;
				Ok(child)
			}
			.await;
			return parent.finish(result).await.map(Some);
		}
		Ok(None)
	}
	/// Check before cached results, provider I/O, and publication after I/O.
	pub(super) async fn require_origins(&self, model: Option<&EntityRef>) -> Result<()> {
		let mut authorities = self.inference.lock().await;
		for OriginLease { run, access } in &mut authorities.leases {
			if let Some(run) = run {
				*access = self.current_origin(*run).await?.ok_or(Error::Forbidden)?;
			}

			access.resume_inherited().await?;
			let current: Result<()> = async {
				if let Some(run) = run {
					self.require_origin_live(access, *run).await?;
				}
				if let Some(reference) = model {
					let resource = crate::authorization::catalog::resource(
						access,
						self.role(reference, "model")?,
					);
					access.require(&resource, "model.infer").await?;
				}
				Ok(())
			}
			.await;
			access.suspend().await?;
			current?;
		}
		Ok(())
	}
	pub(super) async fn reserve_model(
		&self,
		attempt: Uuid,
		input: usize,
		output: u32,
	) -> Result<Option<crate::generation::budget::Reservation>> {
		let mut authorities = self.inference.lock().await;
		let Some(run) = self.run.or_else(|| authorities.runs.first().copied()) else {
			return Ok(None);
		};
		let mut accesses: Vec<_> = authorities
			.leases
			.iter_mut()
			.filter(|origin| origin.run.is_some())
			.map(|origin| &mut origin.access)
			.collect();
		crate::generation::budget::reserve_many(
			&mut accesses,
			&self.store,
			run,
			attempt,
			input,
			output,
		)
		.await
	}
	pub(super) async fn reserve_embedding(
		&self,
		config: &EmbeddingConfig,
		text: &str,
		origin: crate::generation::embedding::Origin,
	) -> Result<Option<crate::generation::embedding::Reservation>> {
		let mut authorities = self.inference.lock().await;
		let mut accesses: Vec<_> = authorities
			.leases
			.iter_mut()
			.map(|origin| &mut origin.access)
			.collect();
		crate::generation::embedding::reserve_many(
			&mut accesses,
			&self.store,
			self.bank.workspace,
			config,
			text,
			origin,
		)
		.await
	}
}
