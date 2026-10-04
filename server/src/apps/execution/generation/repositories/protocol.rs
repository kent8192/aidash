//! Native authority leases and unchanged PostgreSQL queries implement allowance ports.
use crate::{Error, authorization::access::Access, federation::Federation, store::Store};
use aidash_application::{
	Result,
	ports::generation::protocol::{
		GenerationProtocolAuthority, GenerationProtocolLease, GenerationProtocolRepository,
		GenerationProtocolReservation,
	},
};
use aidash_domain::{
	Run,
	federation::execution::Description,
	generation::{
		dispatch::Input,
		remote::{Reserved, Usage},
	},
	semantic::remote::Operation,
};
use async_trait::async_trait;
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct NativeProtocolRepository {
	pub store: Store,
}
#[async_trait]
impl GenerationProtocolRepository for NativeProtocolRepository {
	async fn description(&self, run: Uuid, home: &str) -> Result<Description> {
		let description: Value = {
			let query_bind_1 = run;
			let query_bind_2 = home;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("description"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND source_node=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&self.store.pool)
			.await
			.map_err(Error::from)?
		};
		Ok(serde_json::from_value(description)?)
	}
	async fn run(&self, admission: Uuid) -> Result<Run> {
		self.store.run(admission).await.map_err(Into::into)
	}
	async fn semantic_ready(&self, admission: Uuid, grant: Uuid, step: i32) -> Result<bool> {
		let ready: bool = {
			let query_bind_1 = admission;
			let query_bind_2 = grant;
			let query_bind_3 = step;
			sqlx::query_scalar(&Query::select().expr(Expr::cust("COUNT(*) > 0")).from(Alias::new("semantic_remote_operations"))
                .and_where(SimpleExpr::CustomWithExpr("(admission_id=? AND grant_id=? AND state='READY' AND (binding->'operation'->'boundary'->>'step')::integer=?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
                .to_string(PostgresQueryBuilder)).fetch_one(&self.store.pool).await.map_err(Error::from)?
		};
		Ok(ready)
	}
}

pub(crate) struct NativeProtocolAuthority {
	pub federation: Federation,
}
struct ReservationLease {
	access: Access,
	store: Store,
}
struct Lease {
	reservation: ReservationLease,
	description: Description,
}
#[async_trait]
impl GenerationProtocolReservation for ReservationLease {
	async fn reserve(&mut self, usage: &Usage) -> Result<Vec<Reserved>> {
		aidash_application::generation::reservation::reserve(
			&mut crate::bootstrap::generation_usage_authority_scope(&mut self.access),
			&crate::bootstrap::generation_reservation_repository(&self.store),
			usage,
		)
		.await
	}
	async fn finish_reservations(
		self: Box<Self>,
		result: Result<Vec<Reserved>>,
	) -> Result<Vec<Reserved>> {
		self.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl GenerationProtocolReservation for Lease {
	async fn reserve(&mut self, usage: &Usage) -> Result<Vec<Reserved>> {
		self.reservation.reserve(usage).await
	}
	async fn finish_reservations(
		self: Box<Self>,
		result: Result<Vec<Reserved>>,
	) -> Result<Vec<Reserved>> {
		Box::new(self.reservation).finish_reservations(result).await
	}
}
#[async_trait]
impl GenerationProtocolLease for Lease {
	fn description(&self) -> &Description {
		&self.description
	}
	async fn binding_admission(&mut self, grant: Uuid) -> Result<Option<Uuid>> {
		Ok(
			crate::authorization::remote::execution::binding(&mut self.reservation.access, grant)
				.await?
				.map(|bound| bound.admission_id),
		)
	}
	async fn finish_verification(self: Box<Self>, result: Result<bool>) -> Result<bool> {
		self.reservation
			.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl GenerationProtocolAuthority for NativeProtocolAuthority {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	async fn leaf(
		&self,
		source: &str,
		grant: Uuid,
		admission: Uuid,
	) -> Result<Box<dyn GenerationProtocolLease>> {
		let (access, description) = crate::authorization::peer::admission::leaf_lease(
			&self.federation,
			source,
			grant,
			admission,
		)
		.await?;
		Ok(Box::new(Lease {
			reservation: ReservationLease {
				access,
				store: self.federation.store.clone(),
			},
			description,
		}))
	}
	async fn grant(&self, source: &str, grant: Uuid) -> Result<Box<dyn GenerationProtocolLease>> {
		let (access, description) =
			crate::authorization::remote::description_lease(&self.federation, source, grant)
				.await?;
		Ok(Box::new(Lease {
			reservation: ReservationLease {
				access,
				store: self.federation.store.clone(),
			},
			description,
		}))
	}
	async fn worker(&self, run: &Run) -> Result<Option<Box<dyn GenerationProtocolReservation>>> {
		// Worker admission returns execution configuration, not a grant description.
		// Its accounting lease does not need or reload that separate description.
		Ok(
			crate::authorization::peer::admission::worker_lease(&self.federation, &run.metadata())
				.await?
				.map(|(access, _)| {
					Box::new(ReservationLease {
						access,
						store: self.federation.store.clone(),
					}) as Box<dyn GenerationProtocolReservation>
				}),
		)
	}
	async fn verify_semantic(
		&self,
		description: &Description,
		operation: &Operation,
	) -> Result<()> {
		crate::authorization::peer::semantic::verify_in(&self.federation, description, operation)
			.await
			.map_err(Into::into)
	}
	async fn verify_peer(&self, source: &str, input: &Input) -> Result<bool> {
		crate::authorization::peer::authority_request(
			&self.federation,
			source,
			"/scoped/usage/verify",
			&json!(input),
		)
		.await
		.map_err(Into::into)
	}
	async fn reserve_peer(&self, home: &str, input: &Input) -> Result<Vec<Reserved>> {
		crate::authorization::peer::authority_request(
			&self.federation,
			home,
			"/scoped/usage/reserve",
			&json!(input),
		)
		.await
		.map_err(Into::into)
	}
}
