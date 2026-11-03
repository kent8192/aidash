//! Deployment configuration, retained independently of every Tenant and Node.
//! One pin per Deployment survives native restarts; journal replacement is never auto-accepted.
use crate::{Error, Result, database::native::Pool};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::nodes::DeploymentId;
use chrono::{DateTime, Utc};
use reinhardt::{
	core::exception::Error as FrameworkError,
	db::orm::{Model, execution::convert_values},
	model,
	query::{Alias, IntoValue, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "runner_allocation_journals")]
#[derive(Serialize, Deserialize)]
pub struct RunnerAllocationJournal {
	#[field(primary_key = true, field_type = "text")]
	pub deployment: String,
	#[field(field_type = "uuid")]
	pub runner_instance: Uuid,
	#[field]
	pub pinned_at: DateTime<Utc>,
}

pub(crate) async fn confirm(pool: &Pool, deployment: &DeploymentId, instance: Uuid) -> Result<()> {
	pool.atomic(&TenancyScope::NoTenant, async |tx| {
		// The primary key serializes competing first pins across native replicas.
		let (sql, values) = Query::insert()
			.into_table(Alias::new(RunnerAllocationJournal::table_name()))
			.columns(["deployment", "runner_instance", "pinned_at"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(deployment.as_str()),
				IntoValue::into_value(instance),
				IntoValue::into_value(Utc::now()),
			])
			.on_conflict(
				OnConflict::column(Alias::new("deployment"))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		reinhardt::db::backends::TransactionExecutor::execute(tx, &sql, convert_values(values))
			.await?;
		let pinned = RunnerAllocationJournal::objects()
			.filter(RunnerAllocationJournal::field_deployment().eq(deployment.as_str().to_owned()))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::External("allocation journal pin unavailable".into()))?;
		if pinned.runner_instance != instance {
			return Err(Error::External(
				"allocation journal identity changed; restore the pinned journal".into(),
			));
		}
		Ok(())
	})
	.await
}
#[cfg(test)]
mod tests;
