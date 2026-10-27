//! Native Tenant locks, metadata persistence, and metadata-only lifecycle events.
use super::super::models::provider_credentials::{
	ProviderCredential as Record, ProviderCredentialBinding as BindingRecord,
};
use aidash_application::{
	Error, Result,
	provider_credentials::{Repository, Scope},
};
use aidash_domain::provider_credentials::{Binding, ProviderCredential};
use async_trait::async_trait;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;
#[derive(Clone)]
pub struct NativeRepository {
	pub pool: crate::database::native::Pool,
	pub node: String,
}
struct NativeScope {
	tx: Box<dyn TransactionExecutor>,
	tenant: String,
}
fn contract(row: Record) -> Result<ProviderCredential> {
	Ok(serde_json::from_value(serde_json::to_value(row)?)?)
}
fn binding(row: BindingRecord) -> Result<Binding> {
	Ok(serde_json::from_value(serde_json::to_value(row)?)?)
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
					vec![Expr::value(tenant).into(), Expr::value(71003236_i64).into()],
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
	async fn reconciliation_candidates(
		&self,
		after: Option<Uuid>,
		limit: usize,
	) -> Result<Vec<ProviderCredential>> {
		let scope = aidash_application::tenancy::TenancyScope::Deployment(
			aidash_application::tenancy::DeploymentScopeReason::CredentialReconcile,
		);
		let mut tx = self.pool.begin(&scope).await?.into_executor().await?;
		let mut query = Record::objects()
			.filter(
				Record::field_state()
					.ne("deleted")
					.or(Record::field_pinned_version().is_not_null()),
			)
			.order_by(&["id"])
			.limit(limit.clamp(
				1,
				aidash_application::provider_credentials::RECONCILIATION_BATCH_SIZE,
			));
		if let Some(id) = after {
			query = query.filter(Record::field_id().gt(id));
		}
		let rows = query
			.all_with_executor(tx.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		tx.commit().await.map_err(crate::Error::from)?;
		rows.into_iter().map(contract).collect()
	}
}
#[async_trait]
impl Scope for NativeScope {
	async fn get(&mut self, id: Uuid) -> Result<ProviderCredential> {
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(&self.tenant))
			.filter(Record::field_id().eq(id))
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		contract(
			rows.into_iter()
				.next()
				.ok_or_else(|| Error::NotFound("Provider Credential".into()))?,
		)
	}
	async fn list(&mut self, offset: usize, limit: usize) -> Result<Vec<ProviderCredential>> {
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(&self.tenant))
			.order_by(&["created_at", "id"])
			.offset(offset)
			.limit(limit.clamp(1, 200))
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		rows.into_iter().map(contract).collect()
	}
	async fn count(&mut self) -> Result<usize> {
		Ok(Record::objects()
			.filter(Record::field_tenant().eq(&self.tenant))
			.filter(Record::field_state().ne("deleted"))
			.count_with_executor(self.tx.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)? as usize)
	}
	async fn insert(&mut self, value: &ProviderCredential) -> Result<()> {
		let row: Record = serde_json::from_value(serde_json::to_value(value)?)?;
		Record::objects()
			.insert_with_executor(self.tx.as_mut(), &row)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		Ok(())
	}
	async fn save(&mut self, value: &ProviderCredential, event: &str, actor: &str) -> Result<()> {
		if value.tenant != self.tenant {
			return Err(Error::Forbidden);
		}
		let row: Record = serde_json::from_value(serde_json::to_value(value)?)?;
		Record::objects()
			.save_with_executor(self.tx.as_mut(), &row)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		crate::apps::execution::models::event_records::append(self.tx.as_mut(),None,event,json!({"tenant":value.tenant,"id":value.id,"provider":value.provider,"state":value.state,"revision":value.revision,"actor":actor})).await?;
		Ok(())
	}
	async fn bindings(&mut self) -> Result<Vec<Binding>> {
		let rows = BindingRecord::objects()
			.filter(BindingRecord::field_tenant().eq(&self.tenant))
			.order_by(&["provider"])
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		rows.into_iter().map(binding).collect()
	}
	async fn bind(&mut self, value: &Binding, actor: &str) -> Result<()> {
		let mut json = serde_json::to_value(value)?;
		json["id"] = json!(format!("{}:{}", value.tenant, value.provider.id()));
		let row: BindingRecord = serde_json::from_value(json)?;
		let existing = BindingRecord::objects()
			.filter(BindingRecord::field_id().eq(&row.id))
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		if existing.is_empty() {
			BindingRecord::objects()
				.insert_with_executor(self.tx.as_mut(), &row)
				.await
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(crate::Error::from)?;
		} else {
			BindingRecord::objects()
				.save_with_executor(self.tx.as_mut(), &row)
				.await
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(crate::Error::from)?;
		}
		crate::apps::execution::models::event_records::append(self.tx.as_mut(),None,"provider_credential_binding.updated",json!({"tenant":value.tenant,"id":value.provider.id(),"provider":value.provider,"provider_credential_id":value.provider_credential_id,"state":if value.provider_credential_id.is_some(){"bound"}else{"unbound"},"revision":value.revision,"actor":actor})).await?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Ok(self.tx.commit().await.map_err(crate::Error::from)?)
	}
}

/// Pin all selected provider sources in the caller's admission transaction.
/// Tenant comes from local authorization, including the receiving peer mapping.
/// A peer-admitted Run arrived through Federation, so it never uses a Plan
/// Connection (ADR 0007).
pub async fn admit(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	tenant: &str,
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	configured: bool,
	floor: aidash_domain::data_protection::Level,
	keyless: &aidash_application::provider_access::KeylessAccess,
) -> crate::Result<()> {
	let selected = selected_providers(snapshot)?;
	refuse_plans(&selected)?;
	permit_selected(tx, tenant, floor, keyless, &selected).await?;
	admit_providers(tx, run, tenant, selected.credentials, configured).await
}

/// How a path that admits a Run treats Plan Connection models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanAdmission {
	/// The owner's own admission: the grant it persists next pins the
	/// connection of its delegation-chain root.
	Deferred,
	/// Remote commands, peer and scoped receivers, and Deployment
	/// Administrator claims never use one.
	Refused,
}

/// The Provider Catalog entries one admitted Binding closure calls.
pub(crate) struct Selected {
	/// Served by the Tenant's Provider Credentials.
	pub credentials: std::collections::BTreeSet<String>,
	/// Served by the Plan Connection of the Run's delegation-chain root.
	pub plans: std::collections::BTreeSet<String>,
	pub keyless: Vec<String>,
}

fn refuse_plans(selected: &Selected) -> crate::Result<()> {
	if selected.plans.is_empty() {
		Ok(())
	} else {
		Err(crate::Error::Plan(
			aidash_domain::plan_connections::Failure::NotAdmitted,
		))
	}
}

pub(crate) fn selected_providers(
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
) -> crate::Result<Selected> {
	use aidash_domain::registry::AgentConfig;
	let config = AgentConfig::from_snapshot(snapshot)?;
	// Resolve only the selected local closure that was already admitted. Live
	// Registry overrides and excluded or foreign Bindings cannot retarget a Run.
	let definition = |reference: &aidash_domain::registry::EntityRef| {
		snapshot
			.definitions
			.iter()
			.find(|entry| {
				entry.identity.registry_deployment == snapshot.agent.registry_deployment
					&& entry.identity.id == reference.id
					&& entry.identity.version == reference.version
			})
			.map(|entry| &entry.definition)
			.ok_or_else(|| {
				crate::Error::Invalid(
					"Provider source is absent from the admitted Binding closure".into(),
				)
			})
	};
	let mut definitions = vec![config.model];
	let mut memories: Vec<_> = config.memory.into_iter().collect();
	for source in config.sources {
		let source = definition(&source)?;
		if source.config.get("schema_version").is_none() {
			let source: aidash_domain::memory::SourceConfig =
				serde_json::from_value(source.config.clone())?;
			memories.push(source.memory);
		}
	}
	for memory in memories {
		let entry = definition(&memory)?;
		if entry.config.get("schema_version").is_some() {
			continue;
		}
		let memory: aidash_domain::memory::ProviderConfig =
			serde_json::from_value(entry.config.clone())?;
		definitions.extend([
			memory.policy.extraction,
			memory.policy.derivation,
			memory.policy.reflection,
			memory.policy.embedding,
		]);
		let reranker = definition(&memory.policy.reranker)?;
		if let aidash_domain::memory::RerankerConfig::Model { model } =
			serde_json::from_value(reranker.config.clone())?
		{
			definitions.push(model);
		}
	}
	let mut providers = std::collections::BTreeSet::new();
	let mut plans = std::collections::BTreeSet::new();
	let mut keyless = vec![];
	for (index, reference) in definitions.into_iter().enumerate() {
		let entry = definition(&reference)?;
		if entry
			.config
			.get("provider_credential")
			.is_none_or(serde_json::Value::is_null)
			&& entry
				.config
				.get("plan_connection")
				.is_none_or(serde_json::Value::is_null)
		{
			if let Some(endpoint) = entry
				.config
				.get("endpoint")
				.and_then(serde_json::Value::as_str)
			{
				keyless.push(endpoint.to_owned());
			}
		}
		if let Some(provider) = entry
			.config
			.get("provider_credential")
			.and_then(serde_json::Value::as_str)
		{
			aidash_domain::provider_credentials::validate_source(
				entry.config["endpoint"].as_str().unwrap_or_default(),
				entry.config["provider"].as_str().unwrap_or_default(),
				Some(provider),
			)?;
			providers.insert(provider.to_owned());
		}
		if let Some(provider) = entry
			.config
			.get("plan_connection")
			.and_then(serde_json::Value::as_str)
		{
			// Only the Agent's own model reaches the Responses API. Memory
			// extraction, reflection and reranking run as Tenant maintenance,
			// which never uses a Plan Connection (ADR 0007).
			if index != 0 {
				return Err(crate::Error::Plan(
					aidash_domain::plan_connections::Failure::NotAdmitted,
				));
			}
			aidash_domain::provider_credentials::Provider::plan(provider)?;
			plans.insert(provider.to_owned());
		}
	}
	Ok(Selected {
		credentials: providers,
		plans,
		keyless,
	})
}

async fn admit_providers(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	tenant: &str,
	providers: std::collections::BTreeSet<String>,
	configured: bool,
) -> crate::Result<()> {
	use reinhardt::query::{Alias, IntoValue, OnConflict};
	if !providers.is_empty() && !configured {
		return Err(crate::Error::Invalid(
			"Secret Store is not configured".into(),
		));
	}
	if !providers.is_empty() && tenant.is_empty() {
		return Err(crate::Error::Invalid(
			"Provider Credential admission requires a local Tenant".into(),
		));
	}
	for provider in providers {
		let (lock, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![Expr::value(tenant).into(), Expr::value(71003236_i64).into()],
				)],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&lock, convert_values(values)).await?;
		use super::super::models::provider_credentials::RunProviderCredential as Pin;
		let pins = Pin::objects()
			.filter(Pin::field_run_id().eq(run))
			.filter(Pin::field_provider().eq(&provider))
			.all_with_executor(tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		let credential_id = if let Some(pin) = pins.into_iter().next() {
			if pin.tenant != tenant {
				return Err(crate::Error::Forbidden);
			}
			pin.provider_credential_id
		} else {
			let bindings = BindingRecord::objects()
				.filter(BindingRecord::field_tenant().eq(tenant))
				.filter(BindingRecord::field_provider().eq(&provider))
				.all_with_executor(tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			bindings
				.into_iter()
				.next()
				.and_then(|b| b.provider_credential_id)
				.ok_or_else(|| {
					crate::Error::Invalid("Provider Credential binding is missing".into())
				})?
		};
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(tenant))
			.filter(Record::field_id().eq(credential_id))
			.all_with_executor(tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		let row = contract(rows.into_iter().next().ok_or_else(|| {
			crate::Error::Invalid("bound Provider Credential is unavailable".into())
		})?)?;
		row.require_active()?;
		if row.provider.id() != provider {
			return Err(crate::Error::Invalid(
				"bound Provider Credential provider does not match".into(),
			));
		}
		let (sql, values) = Query::insert()
			.into_table(Alias::new("run_provider_credentials"))
			.columns(
				[
					"id",
					"run_id",
					"tenant",
					"provider",
					"provider_credential_id",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(format!("{run}:{provider}")),
				IntoValue::into_value(run),
				IntoValue::into_value(tenant),
				IntoValue::into_value(&provider),
				IntoValue::into_value(row.id),
			])
			.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(())
}

pub struct AdmittedAccess {
	pub store: crate::store::Store,
}
#[async_trait]
impl aidash_application::provider_access::ProviderAccess for AdmittedAccess {
	async fn data_protection(
		&self,
		context: &aidash_application::provider_access::Context,
	) -> Result<aidash_domain::data_protection::DataProtection> {
		super::data_protection::current(&self.store, context)
			.await
			.map_err(Into::into)
	}
	async fn resolve(
		&self,
		context: &aidash_application::provider_access::Context,
		endpoint: &str,
		source: &aidash_application::provider_access::Source,
	) -> Result<aidash_application::provider_access::Access> {
		crate::apps::execution::observation::provider_scope(&self.store, context, true).await?;
		use aidash_application::provider_access::Source;
		let policy = super::data_protection::current(&self.store, context).await?;
		let destination = match source {
			Source::Plan { .. } => aidash_domain::data_protection::Destination::PlanConnection,
			Source::Tenant { .. } => aidash_domain::data_protection::Destination::OpenRouter,
			Source::Keyless => {
				if self.store.keyless.destination(endpoint)?
					== aidash_domain::keyless::Destination::Permitted
				{
					aidash_domain::data_protection::Destination::DeploymentHosted
				} else {
					aidash_domain::data_protection::Destination::TenantKeyless
				}
			}
		};
		policy.permits(destination)?;
		if let Source::Plan { provider } = source {
			let (service, pin) = self.plan(context, endpoint, provider).await?;
			if policy
				.permits(aidash_domain::data_protection::Destination::PlanConnection)?
				.plan_connection_exemption
			{
				super::data_protection::record_plan_exemption(&self.store, context, policy).await?;
			}
			return Ok(aidash_application::provider_access::Access {
				endpoint: aidash_domain::provider_credentials::Provider::Chatgpt
					.base_url()
					.into(),
				bearer: service.bearer(pin).await?,
				keyless: None,
			});
		}
		let Source::Tenant { provider } = source else {
			let mut context = context.clone();
			context.data_protection = policy;
			return crate::bootstrap::keyless_provider_access(&self.store)
				.resolve(&context, endpoint, source)
				.await;
		};
		let mut context = context.clone();
		if context.inference.as_ref().is_some_and(|inference| {
			inference.operations == [aidash_application::provider_access::Operation::Discovery]
		}) {
			// Catalog reads carry no Tenant content and no body policy to race.
			context.data_protection = policy;
		} else if context.data_protection != policy {
			return Err(Error::Invalid(
				"Data Protection policy changed; retry the call".into(),
			));
		}
		let context = self.scoped(&context, provider).await?;
		self.tenant()?.resolve(&context, endpoint, source).await
	}
	async fn check(
		&self,
		context: &aidash_application::provider_access::Context,
		endpoint: &str,
		source: &aidash_application::provider_access::Source,
	) -> Result<()> {
		crate::apps::execution::observation::provider_scope(&self.store, context, false).await?;
		use aidash_application::provider_access::Source;
		let policy = super::data_protection::current(&self.store, context).await?;
		let destination = match source {
			Source::Plan { .. } => aidash_domain::data_protection::Destination::PlanConnection,
			Source::Tenant { .. } => aidash_domain::data_protection::Destination::OpenRouter,
			Source::Keyless => {
				if self.store.keyless.destination(endpoint)?
					== aidash_domain::keyless::Destination::Permitted
				{
					aidash_domain::data_protection::Destination::DeploymentHosted
				} else {
					aidash_domain::data_protection::Destination::TenantKeyless
				}
			}
		};
		policy.permits(destination)?;
		if let Source::Plan { provider } = source {
			let (service, pin) = self.plan(context, endpoint, provider).await?;
			return service.usable(pin).await;
		}
		let Source::Tenant { provider } = source else {
			let mut context = context.clone();
			context.data_protection = policy;
			return crate::bootstrap::keyless_provider_access(&self.store)
				.check(&context, endpoint, source)
				.await;
		};
		let context = self.scoped(context, provider).await?;
		self.tenant()?.check(&context, endpoint, source).await
	}
	/// One forced refresh after OpenAI rejected the access token, or marking
	/// the owner's plan ineligible (ADR 0007).
	async fn plan_rejected(
		&self,
		context: &aidash_application::provider_access::Context,
		disposition: aidash_domain::plan_connections::Disposition,
	) -> Result<bool> {
		use aidash_domain::plan_connections::Disposition;
		use aidash_domain::provider_credentials::Provider;
		let (service, pin) = self
			.plan(
				context,
				Provider::Chatgpt.base_url(),
				Provider::Chatgpt.id(),
			)
			.await?;
		match disposition {
			Disposition::RefreshThenReconnect => service.refresh_rejected(pin).await,
			Disposition::MarkIneligible => service.mark_ineligible(pin).await.map(|()| false),
			Disposition::Fail(_) | Disposition::Transient => Ok(false),
		}
	}
}
impl AdmittedAccess {
	fn tenant(&self) -> Result<aidash_application::provider_access::TenantAccess> {
		let service = self
			.store
			.provider_credentials
			.as_ref()
			.ok_or_else(|| Error::Invalid("Secret Store is not configured".into()))?;
		Ok(aidash_application::provider_access::TenantAccess {
			keyless: self.store.keyless.clone(),
			repository: service.repository.clone(),
			reader: self.store.provider_key_material_reader.clone(),
			issuer: self.store.capability_issuer.clone(),
		})
	}
	/// The Plan Connection admission pinned for the Run. A Plan Connection
	/// serves only a Run's own inference: never maintenance, and never a
	/// Decision Attempt, which uses the Tenant's OpenRouter binding (ADR 0041).
	async fn plan(
		&self,
		context: &aidash_application::provider_access::Context,
		endpoint: &str,
		provider: &str,
	) -> Result<(
		std::sync::Arc<aidash_application::plan_connections::Service>,
		aidash_domain::plan_connections::Pin,
	)> {
		use aidash_domain::plan_connections::Failure;
		let provider = aidash_domain::provider_credentials::Provider::plan(provider)?;
		let Some(run) = context.run else {
			return Err(aidash_application::Error::Plan(Failure::NotAdmitted));
		};
		if context.maintenance.is_some() || context.is_decision() {
			return Err(aidash_application::Error::Plan(Failure::NotAdmitted));
		}
		if endpoint != provider.base_url() {
			return Err(Error::Invalid(
				"a Plan Connection serves only its Provider Catalog endpoint".into(),
			));
		}
		let service = match (
			self.store.plan_connections_enabled,
			&self.store.plan_connections,
		) {
			(true, Some(service)) => service.clone(),
			_ => {
				return Err(Error::Invalid(
					"Plan Connections are not enabled on this Deployment".into(),
				));
			}
		};
		let pin = super::plan_connections::pinned(
			&self.store.control_pool,
			&context.tenant,
			run,
			provider,
		)
		.await?;
		Ok((service, pin))
	}
	/// The call scope with the Run's admitted Provider Credential pin. Runless
	/// maintenance and a Run's Decision Attempts use the Tenant's current
	/// binding, never the Run's admitted inference pin (ADR 0041).
	async fn scoped(
		&self,
		context: &aidash_application::provider_access::Context,
		provider: &str,
	) -> Result<aidash_application::provider_access::Context> {
		let Some(run) = context.run.filter(|_| !context.is_decision()) else {
			return Ok(context.clone());
		};
		if self.store.provider_credentials.is_none() {
			return Err(Error::Invalid("Secret Store is not configured".into()));
		}
		let scope = aidash_application::tenancy::TenancyScope::tenant(&context.tenant)?;
		let mut tx = self
			.store
			.control_pool
			.begin(&scope)
			.await?
			.into_executor()
			.await?;
		let pins = super::super::models::provider_credentials::RunProviderCredential::objects()
			.filter(
				super::super::models::provider_credentials::RunProviderCredential::field_id()
					.eq(format!("{run}:{provider}")),
			)
			.all_with_executor(tx.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		let pin = pins
			.into_iter()
			.next()
			.ok_or_else(|| Error::Invalid("Run has no admitted Provider Credential pin".into()))?;
		if !context.tenant.is_empty() && context.tenant != pin.tenant {
			return Err(Error::Forbidden);
		}
		tx.commit().await.map_err(crate::Error::from)?;
		Ok(aidash_application::provider_access::Context {
			tenant: pin.tenant,
			data_protection: context.data_protection,
			run: Some(run),
			maintenance: context.maintenance,
			observation: context.observation,
			provider_credential_id: Some(pin.provider_credential_id),
			inference: context.inference.clone(),
		})
	}
}

pub(crate) async fn admit_workspace(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	workspace: Uuid,
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	configured: bool,
	floor: aidash_domain::data_protection::Level,
	keyless: &aidash_application::provider_access::KeylessAccess,
	plan: PlanAdmission,
) -> crate::Result<()> {
	use reinhardt::query::{Alias, ExprTrait};
	let (sql, values) = Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new("authorization_workspaces"))
		.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
		.build(PostgresQueryBuilder);
	let row = tx.fetch_optional(&sql, convert_values(values)).await?;
	let tenant = match row {
		Some(row) => row
			.get::<String>("tenant")
			.map_err(reinhardt::core::exception::Error::from)?,
		None => String::new(),
	};
	let selected = selected_providers(snapshot)?;
	permit_selected(tx, &tenant, floor, keyless, &selected).await?;
	if plan == PlanAdmission::Refused {
		refuse_plans(&selected)?;
	}
	let mut providers = selected.credentials;
	// Workspace retrieval is another admitted provider consumer, even when the
	// Agent's own Model uses an environment source. Lock the index generation
	// with the Run admission so configuration cannot change while pins are made.
	if let Some(index) =
		crate::apps::knowledge::models::SemanticIndexe::locked(tx, workspace, false).await?
	{
		let spec = index.configuration()?;
		if spec.enabled
			&& let Some(provider) = spec.embedding.provider_credential
		{
			if index.tenant != tenant {
				return Err(crate::Error::Forbidden);
			}
			aidash_domain::provider_credentials::validate_source(
				&spec.embedding.endpoint,
				&spec.embedding.provider,
				Some(&provider),
			)?;
			providers.insert(provider);
		}
	}
	admit_providers(tx, run, &tenant, providers, configured).await
}

async fn permit_selected(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	floor: aidash_domain::data_protection::Level,
	origins: &aidash_application::provider_access::KeylessAccess,
	selected: &Selected,
) -> crate::Result<()> {
	use aidash_domain::data_protection::Destination;
	let policy = super::data_protection::stored(tx, tenant)
		.await?
		.with_floor(floor);
	for endpoint in &selected.keyless {
		policy.permits(
			if origins.destination(endpoint)? == aidash_domain::keyless::Destination::Permitted {
				Destination::DeploymentHosted
			} else {
				Destination::TenantKeyless
			},
		)?;
	}
	if !selected.plans.is_empty() {
		policy.permits(Destination::PlanConnection)?;
	}
	Ok(())
}
