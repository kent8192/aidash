//! Retention preserves only scheduled, enabled refreshes over current admitted support.
use super::super::{access::Lease, engine_jobs::Input, units};
use crate::{Error, Result, database::native};
use aidash_domain::{memory::*, registry::EntityRef};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

pub(super) async fn pending(
	lease: &mut Lease<'_>,
	bank: Uuid,
	provider: &EntityRef,
	policy: &Policy,
) -> Result<Vec<Input>> {
	let rows = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
			.and_where(Expr::col("provider_id").eq(provider.id.as_str()))
			.and_where(Expr::col("provider_version").eq(provider.version.as_str()))
			.cond_where(
				Condition::any()
					.add(
						Condition::all()
							.add(Expr::col("state").eq("pending"))
							.add(Expr::col("attempts").lt(policy.bounds.max_retries as i32)),
					)
					.add(
						Condition::all().add(Expr::col("state").eq("running")).add(
							Condition::any()
								.add(Expr::col("attempts").lt(policy.bounds.max_retries as i32))
								.add(
									Condition::all()
										.add(
											Expr::col("next_attempt")
												.gt(Expr::value(chrono::Utc::now())),
										)
										.add(
											Expr::col("attempts")
												.eq(policy.bounds.max_retries as i32),
										),
								),
						),
					),
			)
			.limit(policy.retention.max_model_operations as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if rows.len() > policy.retention.max_model_operations {
		return Err(Error::Conflict(
			"memory repair queue exceeds bank capacity".into(),
		));
	}
	rows.into_iter().map(|row| row.try_get("input")).collect()
}

pub(super) async fn viable(
	lease: &mut Lease<'_>,
	unit: &Unit,
	policy: &Policy,
	jobs: &[Input],
) -> Result<bool> {
	for job in jobs {
		let sources = match job {
			Input::Observation { source, .. }
				if policy.maintain_observations && unit.content.kind == Kind::Observation =>
			{
				let Evidence::Unit { id, .. } = source else {
					continue;
				};
				if crate::semantic::native_memory::request_id(*id, "observation-unit")? != unit.id {
					continue;
				}
				std::slice::from_ref(source)
			}
			Input::MentalModel {
				target,
				revision,
				sources,
				..
			} if policy.refresh_mental_models
				&& *target == unit.id
				&& *revision == unit.revision
				&& unit.content.kind == Kind::MentalModel
				&& unit
					.content
					.mental_model
					.as_ref()
					.is_some_and(|model| model.automatic_refresh) =>
			{
				sources.as_slice()
			}
			_ => continue,
		};
		if sources.is_empty() {
			continue;
		}
		match units::current(
			lease,
			unit.bank.workspace,
			sources,
			policy.bounds.max_graph_visits,
		)
		.await
		{
			Ok(()) => return Ok(true),
			Err(Error::Forbidden | Error::Conflict(_)) => {}
			Err(error) => return Err(error),
		}
	}
	Ok(false)
}
