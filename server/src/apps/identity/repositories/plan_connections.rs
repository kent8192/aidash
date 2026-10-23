//! Native Plan Connection persistence (#159). Every scope holds an advisory
//! lock on one owner, so a rotating refresh token is never raced across
//! server and worker processes. Events carry metadata only.
use super::super::models::{
	DashboardIdentity, DashboardMapping, plan_connections::PlanConnection as Record,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_application::{
	Error, Result,
	plan_connections::{Owner, Repository, Scope},
};
use aidash_domain::plan_connections::{MappedIdentity, Pin, PlanConnection};
use aidash_domain::provider_credentials::Provider;
use async_trait::async_trait;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

/// Distinct from every other advisory lock namespace in the server.
const OWNER_LOCK: i64 = 159_001;

#[derive(Clone)]
pub struct NativeRepository {
	pub pool: crate::database::native::Pool,
	pub node: String,
}
struct NativeScope {
	tx: Box<dyn TransactionExecutor>,
	owner: Owner,
}
fn framework(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	crate::Error::from(error.into()).into()
}
pub(crate) fn contract(row: Record) -> Result<PlanConnection> {
	Ok(serde_json::from_value(serde_json::to_value(row)?)?)
}
fn record(value: &PlanConnection) -> Result<Record> {
	Ok(serde_json::from_value(serde_json::to_value(value)?)?)
}

/// Locks one owner's Plan Connections for the rest of `tx`.
pub(crate) async fn lock_owner(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> crate::Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					// A JSON pair names one owner unambiguously; PostgreSQL text
					// cannot carry the NUL a plain separator would need.
					Expr::value(serde_json::json!([tenant, subject]).to_string()).into(),
					Expr::value(OWNER_LOCK).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// External Identities that hold an enabled Mapping to `subject`, each marked
/// when disabled. A Plan Connection serves calls only while the identity that
/// connected it is the only one not disabled.
pub(crate) async fn mapped_identities(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> crate::Result<Vec<MappedIdentity>> {
	let mut identities: Vec<Uuid> = DashboardMapping::objects()
		.filter(DashboardMapping::field_tenant().eq(tenant))
		.filter(DashboardMapping::field_subject().eq(subject))
		.filter(DashboardMapping::field_enabled().eq(true))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.into_iter()
		.map(|mapping| mapping.identity_id)
		.collect();
	identities.sort_unstable();
	identities.dedup();
	let mut mapped = Vec::with_capacity(identities.len());
	for id in identities {
		// A Mapping always names an existing identity; a missing one counts as
		// disabled, so it never serves or connects anything.
		let disabled = DashboardIdentity::objects()
			.filter(DashboardIdentity::field_id().eq(id))
			.all_with_executor(tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)?
			.pop()
			.is_none_or(|row| row.disabled_at.is_some());
		mapped.push(MappedIdentity { id, disabled });
	}
	Ok(mapped)
}

/// The Plan Connection `subject` holds for `provider`, read in the caller's
/// transaction.
pub(crate) async fn held(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
	provider: Provider,
) -> crate::Result<Option<PlanConnection>> {
	let row = Record::objects()
		.filter(Record::field_tenant().eq(tenant))
		.filter(Record::field_subject().eq(subject))
		.filter(Record::field_provider().eq(provider.id()))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop();
	row.map(contract).transpose().map_err(crate::Error::from)
}

const SHARED_HOLDER: &str = "the subject holds a Plan Connection; it must be disconnected \
	before another Mapping is made to it";

/// Whether `error` is [`refuse_shared_holder`]'s refusal.
pub(crate) fn refused_shared_holder(error: &crate::Error) -> bool {
	matches!(error, crate::Error::Conflict(message) if message == SHARED_HOLDER)
}

/// A second Mapping to a subject that holds a Plan Connection is refused
/// until the connection is disconnected (ADR 0007): otherwise another person
/// could spend the owner's plan. Every path that creates or re-enables a
/// Mapping calls this in its transaction. It takes the owner's lock, which a
/// connection's registration also holds, so a connection registered
/// concurrently is either seen here or sees this Mapping.
pub(crate) async fn refuse_shared_holder(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> crate::Result<()> {
	lock_owner(tx, tenant, subject).await?;
	if held(tx, tenant, subject, Provider::Chatgpt)
		.await?
		.is_some()
	{
		return Err(crate::Error::Conflict(SHARED_HOLDER.into()));
	}
	Ok(())
}

#[async_trait]
impl Repository for NativeRepository {
	async fn begin(&self, owner: &Owner) -> Result<Box<dyn Scope>> {
		let scope = TenancyScope::tenant(&owner.tenant)?;
		if owner.subject.is_empty() || owner.subject.len() > 256 {
			return Err(Error::Invalid("invalid Plan Connection owner".into()));
		}
		let mut tx = self.pool.begin(&scope).await?.into_executor().await?;
		lock_owner(tx.as_mut(), &owner.tenant, &owner.subject).await?;
		Ok(Box::new(NativeScope {
			tx,
			owner: owner.clone(),
		}))
	}
	async fn owner_of(&self, id: Uuid) -> Result<Option<Owner>> {
		let scope = TenancyScope::Deployment(DeploymentScopeReason::PlanConnectionOwner);
		let mut tx = self.pool.begin(&scope).await?.into_executor().await?;
		let row = Record::objects()
			.filter(Record::field_id().eq(id))
			.all_with_executor(tx.as_mut())
			.await
			.map_err(framework)?
			.pop();
		tx.commit().await.map_err(crate::Error::from)?;
		Ok(row.map(|row| Owner {
			tenant: row.tenant,
			subject: row.subject,
		}))
	}
	async fn owners(&self, tenant: &str) -> Result<Vec<Owner>> {
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(tenant)?)
			.await?
			.into_executor()
			.await?;
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(tenant))
			.all_with_executor(tx.as_mut())
			.await
			.map_err(framework)?;
		tx.commit().await.map_err(crate::Error::from)?;
		let mut owners: Vec<_> = rows
			.into_iter()
			.map(|row| (row.tenant, row.subject))
			.collect();
		owners.sort();
		owners.dedup();
		Ok(owners
			.into_iter()
			.map(|(tenant, subject)| Owner { tenant, subject })
			.collect())
	}
}

impl NativeScope {
	fn owns(&self, value: &PlanConnection) -> Result<()> {
		if value.tenant != self.owner.tenant || value.subject != self.owner.subject {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn event(&mut self, value: &PlanConnection, event: &str, actor: &str) -> Result<()> {
		// No workspace stream includes these events; owners read the state
		// through their own connection's endpoint. Never tokens or the client.
		crate::apps::execution::models::event_records::append(
			self.tx.as_mut(),
			None,
			event,
			json!({
				"tenant": value.tenant,
				"subject": value.subject,
				"id": value.id,
				"provider": value.provider,
				"state": value.state,
				"revision": value.revision,
				"actor": actor,
			}),
		)
		.await?;
		Ok(())
	}
}

#[async_trait]
impl Scope for NativeScope {
	async fn owned(&mut self, provider: Provider) -> Result<Option<PlanConnection>> {
		Ok(held(
			self.tx.as_mut(),
			&self.owner.tenant,
			&self.owner.subject,
			provider,
		)
		.await?)
	}
	async fn mapped_identities(&mut self) -> Result<Vec<MappedIdentity>> {
		Ok(mapped_identities(self.tx.as_mut(), &self.owner.tenant, &self.owner.subject).await?)
	}
	async fn insert(&mut self, value: &PlanConnection, event: &str, actor: &str) -> Result<()> {
		self.owns(value)?;
		Record::objects()
			.insert_with_executor(self.tx.as_mut(), &record(value)?)
			.await
			.map_err(framework)?;
		self.event(value, event, actor).await
	}
	async fn save(&mut self, value: &PlanConnection, event: &str, actor: &str) -> Result<()> {
		self.owns(value)?;
		Record::objects()
			.save_with_executor(self.tx.as_mut(), &record(value)?)
			.await
			.map_err(framework)?;
		self.event(value, event, actor).await
	}
	async fn delete(&mut self, value: &PlanConnection, event: &str, actor: &str) -> Result<()> {
		self.owns(value)?;
		Record::objects()
			.delete_with_executor(self.tx.as_mut(), value.id)
			.await
			.map_err(framework)?;
		self.event(value, event, actor).await
	}
	async fn cancel_attempts(&mut self, provider: Provider) -> Result<()> {
		let (sql, values) = Query::delete()
			.from_table(Alias::new(
				super::super::models::plan_connections::PlanAuthorization::table_name(),
			))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(self.owner.tenant.clone())))
			.and_where(Expr::col(Alias::new("subject")).eq(Expr::value(self.owner.subject.clone())))
			.and_where(Expr::col(Alias::new("provider")).eq(Expr::value(provider.id())))
			.build(PostgresQueryBuilder);
		self.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Ok(self.tx.commit().await.map_err(crate::Error::from)?)
	}
}

/// Pins, in the admission transaction, the Plan Connection of the grant's
/// delegation-chain root for every Plan Connection provider the Run's Agent
/// model selects (ADR 0007). A Run whose task arrived through Federation, or
/// whose root holds no usable connection, is refused before it starts.
pub async fn admit(
	tx: &mut dyn TransactionExecutor,
	store: &crate::store::Store,
	grant: &aidash_domain::identity::execution::ExecutionGrant,
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	federated: bool,
) -> crate::Result<()> {
	use aidash_domain::plan_connections::Failure;
	use reinhardt::query::{Alias, IntoValue, OnConflict};
	let plans = super::provider_credentials::selected_providers(snapshot)?.plans;
	if plans.is_empty() {
		return Ok(());
	}
	if federated {
		return Err(crate::Error::Plan(Failure::NotAdmitted));
	}
	if !store.plan_connections_enabled || store.plan_connections.is_none() {
		return Err(crate::Error::Invalid(
			"Plan Connections are not enabled on this Deployment".into(),
		));
	}
	let (tenant, subject) = (&grant.tenant, &grant.root_subject);
	for provider in plans {
		let provider = Provider::plan(&provider)?;
		// A service subject has no Mapping, so it never holds a connection.
		let row = held(tx, tenant, subject, provider)
			.await?
			.ok_or(crate::Error::Plan(Failure::NotConnected))?;
		if !row.serves(&mapped_identities(tx, tenant, subject).await?) {
			return Err(crate::Error::Plan(Failure::NotConnected));
		}
		row.require_usable(chrono::Utc::now())
			.map_err(crate::Error::Plan)?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new("run_plan_connections"))
			.columns(
				[
					"id",
					"run_id",
					"tenant",
					"subject",
					"provider",
					"plan_connection_id",
					"identity_id",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(format!("{}:{}", grant.run_id, provider.id())),
				IntoValue::into_value(grant.run_id),
				IntoValue::into_value(tenant.as_str()),
				IntoValue::into_value(subject.as_str()),
				IntoValue::into_value(provider.id()),
				IntoValue::into_value(row.id),
				IntoValue::into_value(row.identity_id),
			])
			.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(())
}

/// The Plan Connection admission pinned for `run` and `provider`, as the
/// identity it served then.
pub async fn pinned(
	pool: &crate::database::native::Pool,
	tenant: &str,
	run: Uuid,
	provider: Provider,
) -> crate::Result<Pin> {
	use super::super::models::plan_connections::RunPlanConnection as RunPin;
	// A caller without the Run's Tenant finds the pin by the Run alone.
	let scope = if tenant.is_empty() {
		TenancyScope::Deployment(DeploymentScopeReason::RunTenant)
	} else {
		TenancyScope::tenant(tenant)?
	};
	let mut tx = pool.begin(&scope).await?.into_executor().await?;
	let pin = RunPin::objects()
		.filter(RunPin::field_id().eq(format!("{run}:{}", provider.id())))
		.all_with_executor(tx.as_mut())
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop();
	tx.commit().await?;
	let pin = pin.ok_or(crate::Error::Plan(
		aidash_domain::plan_connections::Failure::NotConnected,
	))?;
	if !tenant.is_empty() && pin.tenant != tenant {
		return Err(crate::Error::Forbidden);
	}
	Ok(Pin {
		connection: pin.plan_connection_id,
		identity: pin.identity_id,
	})
}
