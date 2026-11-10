//! The existing remote description query and worker leases implement Summary authorization.
use crate::{apps::identity::repositories::catalog::NativeCatalog, federation::Federation};
use aidash_application::{Result, ports::generation::summary::remote::RemoteSummaryAuthority};
use aidash_domain::Run;
use async_trait::async_trait;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct NativeRemoteSummary<'a> {
	pub catalog: NativeCatalog<'a>,
	pub federation: Federation,
}
#[async_trait]
impl RemoteSummaryAuthority for NativeRemoteSummary<'_> {
	async fn suspend(&mut self) -> Result<()> {
		self.catalog.0.suspend().await.map_err(Into::into)
	}
	async fn refresh(&mut self, run: &Run) -> Result<bool> {
		if let Some((fresh, _)) =
			crate::authorization::peer::admission::worker_lease(&self.federation, &run.metadata())
				.await?
		{
			*self.catalog.0 = fresh;
			Ok(true)
		} else {
			Ok(false)
		}
	}
	async fn description(&mut self, run: Uuid) -> Result<Value> {
		let access = &mut *self.catalog.0;
		let description: Value = {
			let query_bind_1 = run;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("description"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **access.tx)
			.await?
		};

		Ok(description)
	}
}
