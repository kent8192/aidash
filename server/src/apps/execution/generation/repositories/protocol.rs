//! Native authority leases and unchanged PostgreSQL queries implement allowance ports.
use crate::{authorization::access::Access, federation::Federation, store::Store};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
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
	semantic::remote::{Operation, reuse::CitedOperation},
};
use async_trait::async_trait;
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::{Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, SimpleExpr};
use serde::Deserialize as _;
use serde_json::{Value, json};
use uuid::Uuid;

/// A remote Home names an admission, not its local Tenant (#241).
fn inbound() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::FederationInbound)
}

pub(crate) struct NativeProtocolRepository {
	pub store: Store,
}
#[async_trait]
impl GenerationProtocolRepository for NativeProtocolRepository {
	async fn description(&self, run: Uuid, home: &str) -> Result<Description> {
		let description: Value = {
			let query_bind_1 = run;
			let query_bind_2 = home;
			crate::database::native::query_scalar(
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
			.scalar_one(self.store.pool.in_scope(&inbound()))
			.await?
		};
		Ok(serde_json::from_value(description)?)
	}
	async fn run(&self, admission: Uuid) -> Result<Run> {
		self.store
			.run(&inbound(), admission)
			.await
			.map_err(Into::into)
	}
	async fn semantic_ready(&self, admission: Uuid, grant: Uuid, step: i32) -> Result<bool> {
		let ready: bool = {
			let query_bind_1 = admission;
			let query_bind_2 = grant;
			let query_bind_3 = step;
			crate::database::native::query_scalar(&Query::select().expr(Expr::cust("COUNT(*) > 0")).from(Alias::new("semantic_remote_operations"))
                .and_where(SimpleExpr::CustomWithExpr("(admission_id=? AND grant_id=? AND state='READY' AND (binding->'operation'->'boundary'->>'step')::integer=?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
                .to_string(PostgresQueryBuilder)).scalar_one(self.store.pool.in_scope(&inbound())).await?
		};
		Ok(ready)
	}
}

pub(crate) struct NativeProtocolAuthority {
	pub federation: Federation,
	/// The Node an inbound request arrived at; `None` for this Node's own work.
	pub receiver: Option<aidash_domain::nodes::NodeName>,
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
		let tenancy = self.access.tenancy()?;
		aidash_application::generation::reservation::reserve(
			&mut crate::bootstrap::generation_usage_authority_scope(&mut self.access),
			&crate::bootstrap::generation_reservation_repository(&self.store, tenancy),
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
	async fn semantic_sources_current(&mut self, grant: Uuid) -> Result<()> {
		let access = &mut self.reservation.access;
		if !crate::authorization::remote::live(access, grant).await? {
			return Err(aidash_application::Error::Forbidden);
		}
		access
			.remote_semantic_sources(grant)
			.await
			.map_err(Into::into)
	}
	async fn semantic_operation(&mut self, id: Uuid) -> Result<Option<CitedOperation>> {
		const COLUMNS: [&str; 5] = [
			"admission_id",
			"grant_id",
			"state",
			"binding",
			"corpus_digest",
		];
		let row = crate::database::native::query_as::<(Uuid, Uuid, String, Value, Option<String>)>(
			&Query::select()
				.columns(COLUMNS.map(Alias::new))
				.from(Alias::new("semantic_remote_operations"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.columns(&COLUMNS)
		.fetch_optional(&mut **self.reservation.access.tx)
		.await?;
		let Some((admission_id, grant_id, state, binding, corpus_digest)) = row else {
			return Ok(None);
		};
		// The journal binds `{"operation":…,"semantic":…}`; the operation's
		// boundary holds the step it was retrieved at.
		let operation = Operation::deserialize(&binding["operation"])?;
		Ok(Some(CitedOperation {
			admission_id,
			grant_id,
			ready: state == "READY",
			step: operation.boundary.step,
			corpus_digest,
		}))
	}
	async fn corpus_digest(&mut self) -> Result<String> {
		crate::semantic::services::memory_context::remote_corpus_digest(
			self.reservation.access.tx.as_mut(),
			&self.description,
		)
		.await
		.map_err(Into::into)
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
		self.federation.config.deployment.as_str()
	}
	async fn leaf(
		&self,
		source: &str,
		grant: Uuid,
		admission: Uuid,
	) -> Result<Box<dyn GenerationProtocolLease>> {
		let receiver = self
			.receiver
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		let (access, description) = crate::authorization::peer::admission::leaf_lease(
			&self.federation,
			receiver,
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
		let sender = crate::authorization::peer::usage_sender(
			&self.federation,
			input.usage.admission_id,
			input.usage.grant_id,
		)
		.await?;
		crate::authorization::peer::authority_request(
			&self.federation,
			&sender,
			source,
			"/scoped/usage/verify",
			&json!(input),
		)
		.await
		.map_err(Into::into)
	}
	async fn reserve_peer(&self, home: &str, input: &Input) -> Result<Vec<Reserved>> {
		let sender = crate::authorization::peer::usage_sender(
			&self.federation,
			input.usage.admission_id,
			input.usage.grant_id,
		)
		.await?;
		crate::authorization::peer::authority_request(
			&self.federation,
			&sender,
			home,
			"/scoped/usage/reserve",
			&json!(input),
		)
		.await
		.map_err(Into::into)
	}
}
