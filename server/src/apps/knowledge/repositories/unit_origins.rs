//! Current canonical writer authority survives disposable projection rebuilds.
use super::access::Lease;
use crate::{Error, Result, database::native};
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Origin {
	pub revision: i64,
	pub authority: super::super::serializers::service::SavedAuthority,
	pub run: Option<Uuid>,
	pub runs: Vec<Uuid>,
}
pub(crate) async fn load(lease: &mut Lease<'_>, unit: Uuid) -> Result<Option<Origin>> {
	let mut origin: Option<Origin> = native::query(
		&Query::select()
			.columns(["revision", "authority", "origin_run"].map(Alias::new))
			.from(Alias::new("memory_unit_origins"))
			.and_where(Expr::col("unit_id").eq(Expr::value(unit)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	.map(|row| -> Result<Origin> {
		Ok(Origin {
			revision: row.try_get("revision")?,
			authority: serde_json::from_value(row.try_get("authority")?)?,
			run: row.try_get("origin_run")?,
			runs: vec![],
		})
	})
	.transpose()?;
	if let Some(origin) = &mut origin {
		origin.runs = native::query_scalar(
			&Query::select()
				.column(Alias::new("run_id"))
				.from(Alias::new("memory_unit_run_origins"))
				.and_where(Expr::col("unit_id").eq(Expr::value(unit)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut **lease.tx())
		.await?;
		origin.runs.extend(origin.run);
		origin.runs.sort_unstable();
		origin.runs.dedup();
	}
	Ok(origin)
}
pub(crate) async fn record(
	lease: &mut Lease<'_>,
	unit: &aidash_domain::memory::Unit,
	run: Option<Uuid>,
	max_origins: Option<usize>,
) -> Result<()> {
	let previous = load(lease, unit.id).await?;
	let origin = if unit.stale || unit.deleted {
		previous.ok_or(Error::Forbidden)?
	} else {
		let bound = max_origins.ok_or(Error::Forbidden)?;
		let mut runs: std::collections::BTreeSet<Uuid> = previous
			.as_ref()
			.into_iter()
			.filter(|_| !unit.content.kind.derived())
			.flat_map(|origin| origin.runs.iter().copied())
			.collect();
		runs.extend(run);
		for proof in &unit.content.evidence {
			match proof {
				aidash_domain::memory::Evidence::Run { id, .. } => {
					runs.insert(*id);
				}
				aidash_domain::memory::Evidence::Unit { id, revision, .. } => {
					let source = load(lease, *id).await?.ok_or(Error::Forbidden)?;
					if source.revision != *revision {
						return Err(Error::Forbidden);
					}
					runs.extend(source.runs);
				}
				aidash_domain::memory::Evidence::Publication { id, revision } => {
					// Publication changes disclosure, not the origins that must fund
					// future synthesis/indexing of the selected private knowledge.
					let publication = native::query(
						&Query::select()
							.columns(["source_id", "source_revision"].map(Alias::new))
							.from(Alias::new("memory_publications"))
							.and_where(Expr::col("id").eq(Expr::value(*id)))
							.and_where(Expr::col("revision").eq(*revision))
							.and_where(Expr::col("deleted").eq(false))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_optional(&mut **lease.tx())
					.await?
					.ok_or(Error::Forbidden)?;
					let source = load(lease, publication.try_get("source_id")?)
						.await?
						.ok_or(Error::Forbidden)?;
					if source.revision != publication.try_get::<i64>("source_revision")? {
						return Err(Error::Forbidden);
					}
					runs.extend(source.runs);
				}
				_ => {}
			}
			if runs.len() > bound {
				return Err(Error::Conflict(
					"memory origin lineage exceeds its allowance".into(),
				));
			}
		}
		if runs.len() > bound {
			return Err(Error::Conflict(
				"memory origin lineage exceeds its allowance".into(),
			));
		}
		Origin {
			revision: unit.revision,
			authority: serde_json::from_value(lease.saved()?)?,
			run,
			runs: runs.into_iter().collect(),
		}
	};
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_unit_origins"))
			.columns(["unit_id", "revision", "authority", "origin_run"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(unit.id))
					.expr(Expr::value(unit.revision))
					.expr(Expr::value(serde_json::to_value(origin.authority)?))
					.expr(Expr::value(origin.run))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("unit_id"))
					.update_columns(["revision", "authority", "origin_run"].map(Alias::new))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	if unit.visible() && unit.content.kind.derived() {
		// A refreshed observation depends on its current surviving support. Old
		// source origins remain in canonical history, not in the new I/O lease.
		native::query(
			&Query::delete()
				.from_table(Alias::new("memory_unit_run_origins"))
				.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	for run in origin.runs {
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_unit_run_origins"))
				.columns(["unit_id", "run_id"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value(unit.id))
						.expr(Expr::value(run))
						.to_owned(),
				)
				.on_conflict(
					OnConflict::columns(["unit_id", "run_id"].map(Alias::new))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	Ok(())
}
