//! One authoritative sender schedule per Deployment. Optimistic compare-and-set
//! serializes independent management commands without a network call inside a DB lock.
use crate::{Error, Result, database::native::Pool};
use aidash_application::{
	federation::key::{DeploymentKeyRollover, DeploymentKeyStore},
	tenancy::TenancyScope,
};
use aidash_domain::nodes::DeploymentId;
use async_trait::async_trait;
use reinhardt::{
	db::orm::execution::convert_values,
	query::{
		Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	},
};

pub(crate) struct PostgresDeploymentKeys {
	pub pool: Pool,
	pub deployment: DeploymentId,
}
#[async_trait]
impl DeploymentKeyStore for PostgresDeploymentKeys {
	async fn load(&self) -> aidash_application::Result<Option<DeploymentKeyRollover>> {
		Ok(self.read().await?)
	}
	async fn compare_and_set(
		&self,
		expected: Option<&DeploymentKeyRollover>,
		next: &DeploymentKeyRollover,
	) -> aidash_application::Result<bool> {
		Ok(self.replace(expected, next).await?)
	}
}
impl PostgresDeploymentKeys {
	async fn read(&self) -> Result<Option<DeploymentKeyRollover>> {
		let mut tx = self.pool.begin(&TenancyScope::NoTenant).await?;
		let (sql, values) = Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("deployment_key_rollovers"))
			.and_where(Expr::col("deployment").eq(Expr::value(self.deployment.to_string())))
			.build(PostgresQueryBuilder);
		let row = tx.fetch_optional(&sql, convert_values(values)).await?;
		let state = row
			.map(|row| -> Result<DeploymentKeyRollover> {
				let text: String = row
					.get("state")
					.map_err(reinhardt::core::exception::Error::from)?;
				let state: DeploymentKeyRollover = serde_json::from_str(&text)
					.map_err(|_| Error::Invalid("invalid stored Deployment Key schedule".into()))?;
				state.validate(&self.deployment)?;
				Ok(state)
			})
			.transpose()?;
		tx.commit().await?;
		Ok(state)
	}
	async fn replace(
		&self,
		expected: Option<&DeploymentKeyRollover>,
		next: &DeploymentKeyRollover,
	) -> Result<bool> {
		next.validate(&self.deployment)?;
		let next = serde_json::to_string(next)?;
		let (sql, values) = match expected {
			Some(expected) => {
				expected.validate(&self.deployment)?;
				Query::update()
					.table(Alias::new("deployment_key_rollovers"))
					.value(Alias::new("state"), Expr::value(next))
					.and_where(Expr::col("deployment").eq(Expr::value(self.deployment.to_string())))
					.and_where(Expr::col("state").eq(Expr::value(serde_json::to_string(expected)?)))
					.build(PostgresQueryBuilder)
			}
			None => Query::insert()
				.into_table(Alias::new("deployment_key_rollovers"))
				.columns([Alias::new("deployment"), Alias::new("state")])
				.values_panic([Expr::value(self.deployment.to_string()), Expr::value(next)])
				.on_conflict(
					OnConflict::column(Alias::new("deployment"))
						.do_nothing()
						.to_owned(),
				)
				.build(PostgresQueryBuilder),
		};
		let mut tx = self.pool.begin(&TenancyScope::NoTenant).await?;
		let changed = tx.execute(&sql, convert_values(values)).await?;
		tx.commit().await?;
		Ok(changed.rows_affected == 1)
	}
}

#[cfg(test)]
#[path = "../tests/deployment_keys.rs"]
mod tests;
