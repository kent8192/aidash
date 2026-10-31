//! Tenant-serialized OAuth Client and ceiling persistence through Reinhardt.
use super::super::models::oauth_clients::{
	OAuthClient as ClientRecord, ScopeCeiling as CeilingRecord,
};
use aidash_application::{
	Error, Result,
	oauth_clients::{Repository, Scope, ScopeCeiling},
};
use aidash_domain::oauth_connections::OAuthClient;
use async_trait::async_trait;
use reinhardt::{
	db::{
		backends::TransactionExecutor,
		orm::{Model, execution::convert_values},
	},
	query::{Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr},
};

#[derive(Clone)]
pub struct NativeRepository {
	pub pool: crate::database::native::Pool,
}
struct NativeScope {
	tx: Box<dyn TransactionExecutor>,
	tenant: String,
}
fn failed(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	crate::Error::from(error.into()).into()
}
#[async_trait]
impl Repository for NativeRepository {
	async fn begin(&self, tenant: &str) -> Result<Box<dyn Scope>> {
		let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
		let mut tx = self.pool.begin(&scope).await?.into_executor().await?;
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![Expr::value(tenant).into(), Expr::value(554_002_i64).into()],
				)],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		Ok(Box::new(NativeScope {
			tx,
			tenant: tenant.into(),
		}))
	}
}
#[async_trait]
impl Scope for NativeScope {
	async fn clients(&mut self) -> Result<Vec<OAuthClient>> {
		let rows = ClientRecord::objects()
			.filter(ClientRecord::field_tenant().eq(&self.tenant))
			.order_by(&["issuer", "id"])
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(failed)?;
		rows.into_iter()
			.map(|row| Ok(serde_json::from_value(serde_json::to_value(row)?)?))
			.collect()
	}
	async fn save_client(&mut self, row: &OAuthClient, new: bool) -> Result<()> {
		if row.tenant != self.tenant {
			return Err(Error::Forbidden);
		}
		let record: ClientRecord = serde_json::from_value(serde_json::to_value(row)?)?;
		if new {
			ClientRecord::objects()
				.insert_with_executor(self.tx.as_mut(), &record)
				.await
				.map_err(failed)?;
		} else {
			ClientRecord::objects()
				.save_with_executor(self.tx.as_mut(), &record)
				.await
				.map_err(failed)?;
		}
		Ok(())
	}
	async fn remove_client(&mut self, id: uuid::Uuid) -> Result<()> {
		ClientRecord::objects()
			.delete_with_executor(self.tx.as_mut(), id)
			.await
			.map_err(failed)?;
		Ok(())
	}
	async fn ceiling(&mut self, resource: &str) -> Result<Option<ScopeCeiling>> {
		let row = CeilingRecord::objects()
			.filter(CeilingRecord::field_tenant().eq(&self.tenant))
			.filter(CeilingRecord::field_resource().eq(resource))
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(failed)?
			.into_iter()
			.next();
		row.map(|row| Ok(serde_json::from_value(serde_json::to_value(row)?)?))
			.transpose()
	}
	async fn save_ceiling(&mut self, row: &ScopeCeiling, new: bool) -> Result<()> {
		if row.tenant != self.tenant {
			return Err(Error::Forbidden);
		}
		let record: CeilingRecord = serde_json::from_value(serde_json::to_value(row)?)?;
		if new {
			CeilingRecord::objects()
				.insert_with_executor(self.tx.as_mut(), &record)
				.await
				.map_err(failed)?;
		} else {
			CeilingRecord::objects()
				.save_with_executor(self.tx.as_mut(), &record)
				.await
				.map_err(failed)?;
		}
		Ok(())
	}
	async fn remove_ceiling(&mut self, resource: &str) -> Result<()> {
		// The pinned Manager delete binds only one primary-key column.
		// Keep both predicates in the ORM query, as Secret Store versions do.
		let query = CeilingRecord::objects()
			.filter(CeilingRecord::field_tenant().eq(&self.tenant))
			.filter(CeilingRecord::field_resource().eq(resource))
			.delete_query()
			.map_err(failed)?;
		let (sql, values) = query.build(PostgresQueryBuilder);
		self.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.tx.commit().await.map_err(crate::Error::from)?;
		Ok(())
	}
}
