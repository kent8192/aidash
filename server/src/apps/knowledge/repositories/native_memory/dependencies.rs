//! Reserve complete reverse impact while forward provenance is admitted.
use super::*;

pub(super) async fn dependents(
	lease: &mut Lease<'_>,
	id: Uuid,
	cap: usize,
) -> Result<Vec<native::Row>> {
	let rows = native::query(
		&Query::select()
			.distinct()
			.columns([("d", "unit_id"), ("d", "source_revision")])
			.from_as(Alias::new("memory_dependencies"), Alias::new("d"))
			.inner_join(
				Alias::new("memory_units"),
				Expr::col(("d", "unit_id")).equals(("memory_units", "id")),
			)
			.and_where(Expr::col(("d", "source_kind")).eq("unit"))
			.and_where(Expr::col(("d", "source_id")).eq(Expr::value(id)))
			.and_where(Expr::col(("memory_units", "deleted")).eq(false))
			.and_where(Expr::col(("memory_units", "stale")).eq(false))
			.order_by(("d", "unit_id"), Order::Asc)
			.limit(cap as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if rows.len() > cap {
		return Err(Error::Conflict(
			"memory dependency impact exceeds its declared bound".into(),
		));
	}
	Ok(rows)
}

pub(crate) async fn validate_impact(lease: &mut Lease<'_>, root: Uuid, cap: usize) -> Result<()> {
	let mut pending = VecDeque::from([root]);
	let mut seen = BTreeSet::from([root]);
	while let Some(id) = pending.pop_front() {
		for row in dependents(lease, id, cap).await? {
			let dependent = row.try_get("unit_id")?;
			if seen.insert(dependent) {
				if seen.len() > cap {
					return Err(Error::Conflict(
						"memory dependency impact exceeds its declared bound".into(),
					));
				}
				pending.push_back(dependent);
			}
		}
	}
	Ok(())
}

pub(super) async fn validate_admission(
	lease: &mut Lease<'_>,
	unit: &Unit,
	bounds: &Bounds,
) -> Result<()> {
	let mut pending = VecDeque::from([unit.id]);
	let mut seen = BTreeSet::new();
	while let Some(id) = pending.pop_front() {
		if !seen.insert(id) {
			continue;
		}
		// Forward provenance has a separate cap; reserve its root as well.
		if seen.len() > bounds.max_graph_visits.saturating_add(1) {
			return Err(Error::Conflict(
				"memory dependency ancestry exceeds its declared bound".into(),
			));
		}
		let source = units::load(lease, id, false)
			.await?
			.ok_or(Error::Forbidden)?;
		let settings = super::super::bank_settings::get(lease, &source.bank)
			.await?
			.ok_or(Error::Forbidden)?;
		let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
		validate_impact(lease, id, policy.bounds.max_graph_visits).await?;
		let ancestors: Vec<Uuid> = native::query_scalar(
			&Query::select()
				.distinct()
				.column(Alias::new("source_id"))
				.from(Alias::new("memory_dependencies"))
				.and_where(Expr::col("unit_id").eq(Expr::value(id)))
				.and_where(Expr::col("source_kind").eq("unit"))
				.limit(bounds.max_graph_visits as u64 + 1)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut **lease.tx())
		.await?;
		pending.extend(ancestors);
	}
	Ok(())
}
