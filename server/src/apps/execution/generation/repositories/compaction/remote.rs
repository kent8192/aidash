//! The existing remote description query and worker leases implement compaction ports.
use crate::{Error, apps::identity::repositories::catalog::NativeCatalog, federation::Federation};
use aidash_application::{
	Result,
	ports::{catalog::CatalogScope, generation::compaction::remote::RemoteCompactionAuthority},
};
use aidash_domain::{
	Run,
	generation::{
		dispatch::Input,
		remote::{Finalization, Purpose},
	},
};
use async_trait::async_trait;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct NativeRemoteCompaction<'a> {
	pub catalog: NativeCatalog<'a>,
	pub federation: Federation,
}
#[async_trait]
impl RemoteCompactionAuthority for NativeRemoteCompaction<'_> {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	fn catalog(&mut self) -> &mut dyn CatalogScope {
		&mut self.catalog
	}
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
			sqlx::query_scalar(
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
			.fetch_one(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};

		Ok(description)
	}
	async fn admit(
		&mut self,
		run: &Run,
		attempt: Uuid,
		input_digest: String,
		amount: i64,
	) -> Result<Input> {
		let federation = &self.federation;
		aidash_application::generation::protocol::admit(
			&crate::bootstrap::generation_protocol_authority(federation),
			&crate::bootstrap::generation_protocol_repository(&federation.store),
			&crate::bootstrap::generation_dispatch_repository(&federation.store),
			&crate::bootstrap::generation_dispatch_settlement(federation),
			aidash_application::generation::protocol::Admission {
				run,
				attempt,
				purpose: Purpose::Compaction,
				input_digest,
				amount,
			},
		)
		.await
	}
	async fn settle(&mut self, input: &Input) -> Result<()> {
		aidash_application::generation::dispatch::finish(
			&crate::bootstrap::generation_dispatch_repository(&self.federation.store),
			&crate::bootstrap::generation_dispatch_settlement(&self.federation),
			input,
			Finalization::Settled { reported: None },
		)
		.await
	}
}
