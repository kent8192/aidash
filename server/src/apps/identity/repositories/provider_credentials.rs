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
	node: String,
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
		aidash_domain::policy::identifier(tenant)?;
		let mut tx = self
			.pool
			.connection()
			.begin()
			.await
			.map_err(crate::Error::from)?;
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
			node: self.node.clone(),
		}))
	}
	async fn pending(&self) -> Result<Vec<ProviderCredential>> {
		let mut db = self
			.pool
			.connection()
			.begin()
			.await
			.map_err(crate::Error::from)?;
		let rows = Record::objects()
			.filter(Record::field_state().eq("pending"))
			.order_by(&["created_at", "id"])
			.limit(200)
			.all_with_executor(db.as_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		rows.into_iter().map(contract).collect()
	}
	async fn active(&self) -> Result<Vec<ProviderCredential>> {
		let mut tx = self
			.pool
			.connection()
			.begin()
			.await
			.map_err(crate::Error::from)?;
		let rows = Record::objects()
			.filter(Record::field_state().eq("active"))
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
		crate::apps::execution::models::event_records::append(self.tx.as_mut(),&self.node,None,event,json!({"tenant":value.tenant,"id":value.id,"provider":value.provider,"state":value.state,"revision":value.revision,"actor":actor})).await?;
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
		crate::apps::execution::models::event_records::append(self.tx.as_mut(),&self.node,None,"provider_credential_binding.updated",json!({"tenant":value.tenant,"id":value.provider.id(),"provider":value.provider,"provider_credential_id":value.provider_credential_id,"state":if value.provider_credential_id.is_some(){"bound"}else{"unbound"},"revision":value.revision,"actor":actor})).await?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Ok(self.tx.commit().await.map_err(crate::Error::from)?)
	}
}

/// Pin all selected provider sources in the caller's admission transaction.
/// Tenant comes from local authorization, including the receiving peer mapping.
pub async fn admit(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	tenant: &str,
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	configured: bool,
) -> crate::Result<()> {
	admit_providers(tx, run, tenant, selected_providers(snapshot)?, configured).await
}

fn selected_providers(
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
) -> crate::Result<std::collections::BTreeSet<String>> {
	use aidash_domain::registry::AgentConfig;
	let config = AgentConfig::from_snapshot(snapshot)?;
	// Resolve only the selected local closure that was already admitted. Live
	// Registry overrides and excluded or foreign Bindings cannot retarget a Run.
	let definition = |reference: &aidash_domain::registry::EntityRef| {
		snapshot
			.definitions
			.iter()
			.find(|entry| {
				entry.identity.registry_node == snapshot.agent.registry_node
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
	for reference in definitions {
		let entry = definition(&reference)?;
		if let Some(provider) = entry
			.config
			.get("provider_credential")
			.and_then(serde_json::Value::as_str)
		{
			aidash_domain::provider_credentials::validate_source(
				entry.config["endpoint"].as_str().unwrap_or_default(),
				entry.config["provider"].as_str().unwrap_or_default(),
				entry.config["credential_env"].as_str(),
				Some(provider),
			)?;
			providers.insert(provider.to_owned());
		}
	}
	Ok(providers)
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
			"Provider Credential Store is not configured".into(),
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
	async fn resolve(
		&self,
		context: &aidash_application::provider_access::Context,
		endpoint: &str,
		source: &aidash_application::provider_access::Source,
	) -> Result<aidash_application::provider_access::Access> {
		use aidash_application::provider_access::{EnvironmentAccess, Source, TenantAccess};
		let Source::Tenant { provider } = source else {
			return crate::bootstrap::environment_provider_access()
				.resolve(context, endpoint, source)
				.await;
		};
		let service =
			self.store.provider_credentials.as_ref().ok_or_else(|| {
				Error::Invalid("Provider Credential Store is not configured".into())
			})?;
		if context.run.is_none() {
			if context.tenant.is_empty() || context.maintenance.is_none() {
				return Err(Error::Invalid(
					"Provider Credential requires a local Tenant and maintenance purpose".into(),
				));
			}
			let provider = aidash_domain::provider_credentials::Provider::parse(provider)?;
			let mut scope = service.repository.begin(&context.tenant).await?;
			let id = scope
				.bindings()
				.await?
				.into_iter()
				.find(|binding| binding.provider == provider)
				.and_then(|binding| binding.provider_credential_id)
				.ok_or_else(|| Error::Invalid("Provider Credential binding is missing".into()))?;
			scope.commit().await?;
			let mut resolved = context.clone();
			resolved.provider_credential_id = Some(id);
			return TenantAccess {
				environment: EnvironmentAccess {
					credentials: crate::bootstrap::environment_credentials(),
				},
				repository: service.repository.clone(),
				reader: self.store.provider_key_material_reader.clone(),
			}
			.resolve(&resolved, endpoint, source)
			.await;
		}
		let run = context.run.expect("Run was checked above");
		let mut tx = self
			.store
			.control_pool
			.connection()
			.begin()
			.await
			.map_err(crate::Error::from)?;
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
		let context = aidash_application::provider_access::Context {
			tenant: pin.tenant,
			run: Some(run),
			maintenance: context.maintenance,
			provider_credential_id: Some(pin.provider_credential_id),
		};
		TenantAccess {
			environment: EnvironmentAccess {
				credentials: crate::bootstrap::environment_credentials(),
			},
			repository: service.repository.clone(),
			reader: self.store.provider_key_material_reader.clone(),
		}
		.resolve(&context, endpoint, source)
		.await
	}
}

pub(crate) async fn admit_workspace(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	workspace: Uuid,
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	configured: bool,
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
	let mut providers = selected_providers(snapshot)?;
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
				spec.embedding.credential_env.as_deref(),
				Some(&provider),
			)?;
			providers.insert(provider);
		}
	}
	admit_providers(tx, run, &tenant, providers, configured).await
}
