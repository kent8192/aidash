//! Policy persistence and per-send reads. Policies are never pinned by a Binding.
use super::super::models::{TenantDataProtection, TenantDataProtectionHistory};
use crate::{Error, Result, store::Store};
use aidash_application::provider_access::Context;
use aidash_application::tenancy::TenancyScope;
use aidash_domain::data_protection::{DataProtection, Level};
use chrono::Utc;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub(crate) async fn stored(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<DataProtection> {
	let rows = TenantDataProtection::objects()
		.filter(TenantDataProtection::field_tenant().eq(tenant.to_owned()))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	rows.into_iter()
		.next()
		.map(|row| {
			let level: Level = serde_json::from_value(serde_json::Value::String(row.level))?;
			let policy = DataProtection {
				level,
				model_call_zdr: row.model_call_zdr,
				plan_connection_exempt: row.plan_connection_exempt,
			};
			policy.validate()?;
			Ok(policy)
		})
		.unwrap_or_else(|| Ok(DataProtection::default()))
}

pub(crate) async fn current(store: &Store, context: &Context) -> Result<DataProtection> {
	use aidash_application::tenancy::DeploymentScopeReason;
	let mut tenants = std::collections::BTreeSet::new();
	if !context.tenant.is_empty() {
		tenants.insert(context.tenant.clone());
	}
	if let Some(run) = context.run {
		let scope = TenancyScope::Deployment(DeploymentScopeReason::DataProtectionEvaluation);
		let mut tx = store
			.control_pool
			.begin(&scope)
			.await?
			.into_executor()
			.await?;
		let mut cursor = Some(run);
		let mut visited = std::collections::BTreeSet::new();
		while let Some(run) = cursor {
			if visited.len() >= 128 || !visited.insert(run) {
				return Err(Error::Invalid(
					"Data Protection: delegation ancestry is incomplete or cyclic".into(),
				));
			}
			let (sql, values) = Query::select()
				.columns(["tenant", "task_id"].map(Alias::new))
				.from(Alias::new("runs"))
				.and_where(Expr::col("id").eq(Expr::value(run)))
				.build(PostgresQueryBuilder);
			let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? else {
				// Admission may check a context before its new Run has been persisted.
				if visited.len() == 1 && !tenants.is_empty() {
					break;
				}
				return Err(Error::NotFound("Data Protection Run ancestry".into()));
			};
			let tenant: String = row
				.get("tenant")
				.map_err(reinhardt::core::exception::Error::from)?;
			if visited.len() == 1 && !context.tenant.is_empty() && tenant != context.tenant {
				return Err(Error::Forbidden);
			}
			tenants.insert(tenant);
			let task: uuid::Uuid = row
				.get("task_id")
				.map_err(reinhardt::core::exception::Error::from)?;
			let (sql, values) = Query::select()
				.column(Alias::new("source_run_id"))
				.from(Alias::new("authorization_task_origins"))
				.and_where(Expr::col("task_id").eq(Expr::value(task)))
				.build(PostgresQueryBuilder);
			cursor = tx
				.fetch_optional(&sql, convert_values(values))
				.await?
				.map(|row| {
					row.get("source_run_id")
						.map_err(reinhardt::core::exception::Error::from)
				})
				.transpose()?;
		}
		let mut policies = vec![];
		for tenant in tenants {
			policies.push(stored(tx.as_mut(), &tenant).await?);
		}
		tx.commit().await?;
		return Ok(DataProtection::combine(
			policies,
			store.data_protection_floor,
		));
	}
	if tenants.is_empty() {
		return Ok(context
			.data_protection
			.with_floor(store.data_protection_floor));
	}
	let tenant = tenants.first().expect("nonempty tenant set");
	let mut tx = store
		.control_pool
		.begin(&TenancyScope::tenant(tenant)?)
		.await?
		.into_executor()
		.await?;
	let policy = stored(tx.as_mut(), tenant).await?;
	tx.commit().await?;
	Ok(policy.with_floor(store.data_protection_floor))
}

pub(crate) async fn save(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	policy: DataProtection,
	by: &str,
) -> Result<()> {
	policy.validate()?;
	let level = serde_json::to_value(policy.level)?
		.as_str()
		.expect("level string")
		.to_owned();
	let now = Utc::now();
	let (sql, values) = Query::delete()
		.from_table(Alias::new(TenantDataProtection::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let row = TenantDataProtection::build()
		.tenant(tenant.to_owned())
		.level(level.clone())
		.model_call_zdr(policy.model_call_zdr)
		.plan_connection_exempt(policy.plan_connection_exempt)
		.updated_at(now)
		.updated_by(by.to_owned())
		.finish();
	TenantDataProtection::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	let change = TenantDataProtectionHistory::build()
		.id(uuid::Uuid::now_v7())
		.tenant(tenant.to_owned())
		.level(level)
		.model_call_zdr(policy.model_call_zdr)
		.plan_connection_exempt(policy.plan_connection_exempt)
		.changed_at(now)
		.changed_by(by.to_owned())
		.finish();
	TenantDataProtectionHistory::objects()
		.insert_with_executor(tx, &change)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	Ok(())
}

pub(crate) async fn impact(
	tx: &mut dyn TransactionExecutor,
	store: &Store,
	tenant: &str,
	policy: DataProtection,
) -> Result<super::super::serializers::data_protection::Impact> {
	use super::super::serializers::data_protection::{Affected, Impact};
	use aidash_domain::data_protection::Destination;
	let (sql, values) = Query::select()
		.expr_as(
			Expr::col("metadata").cast_as(Alias::new("text")),
			Alias::new("metadata_json"),
		)
		.from(Alias::new("registry"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.build(PostgresQueryBuilder);
	let rows = tx.fetch_all(&sql, convert_values(values)).await?;
	let research =
		crate::apps::execution::services::research::providers().data_protection_denials(policy);
	let mut blocked = std::collections::BTreeMap::new();
	for row in rows {
		let entry: aidash_domain::registry::Entry = serde_json::from_str(
			&row.get::<String>("metadata_json")
				.map_err(reinhardt::core::exception::Error::from)?,
		)?;
		if entry.kind == "tool"
			&& let Some(reason) = entry
				.config
				.get("provider")
				.and_then(serde_json::Value::as_str)
				.and_then(|provider| research.get(provider))
		{
			blocked.insert((entry.id.clone(), entry.version.clone()), reason.clone());
		}
		if entry.kind == "model" {
			let config: aidash_domain::model::ModelConfig = serde_json::from_value(entry.config)?;
			let destination = if config.plan_connection.is_some() {
				Destination::PlanConnection
			} else if config.provider_credential.is_some() {
				Destination::OpenRouter
			} else if store.keyless.destination(&config.endpoint)?
				== aidash_domain::keyless::Destination::Permitted
			{
				Destination::DeploymentHosted
			} else {
				Destination::TenantKeyless
			};
			if let Err(error) = policy.permits(destination) {
				blocked.insert((entry.id, entry.version), error.to_string());
			}
		}
	}
	let mut impact = Impact {
		effective: policy,
		agents: vec![],
		catalog_approvals: vec![],
		destinations: blocked
			.iter()
			.map(|((id, version), reason)| Affected {
				id: id.clone(),
				version: Some(version.clone()),
				reason: reason.clone(),
			})
			.collect(),
	};
	let approvals = super::super::models::AuthorizationCatalog::objects()
		.filter(super::super::models::AuthorizationCatalog::field_tenant().eq(tenant.to_owned()))
		.filter(super::super::models::AuthorizationCatalog::field_enabled().eq(true))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	for approval in approvals {
		if let Some(reason) =
			blocked.get(&(approval.entry_key.clone(), approval.entry_version.clone()))
		{
			impact.catalog_approvals.push(Affected {
				id: format!("{}:{}", approval.node, approval.entry_key),
				version: Some(approval.entry_version),
				reason: reason.clone(),
			});
		}
	}
	let (sql, values) = Query::select()
		.columns(["agent_id", "agent_version", "model_id", "model_version"].map(Alias::new))
		.from(Alias::new("registry_agent_model_refs"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.build(PostgresQueryBuilder);
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let model = (
			row.get::<String>("model_id")
				.map_err(reinhardt::core::exception::Error::from)?,
			row.get::<String>("model_version")
				.map_err(reinhardt::core::exception::Error::from)?,
		);
		if let Some(reason) = blocked.get(&model) {
			impact.agents.push(Affected {
				id: row
					.get("agent_id")
					.map_err(reinhardt::core::exception::Error::from)?,
				version: Some(
					row.get("agent_version")
						.map_err(reinhardt::core::exception::Error::from)?,
				),
				reason: reason.clone(),
			});
		}
	}
	for (id, reason) in research {
		impact.destinations.push(Affected {
			id,
			version: None,
			reason,
		});
	}
	let (sql, values) = Query::select()
		.columns(
			[
				"agent_id",
				"agent_version",
				"reference_id",
				"reference_version",
			]
			.map(Alias::new),
		)
		.from(Alias::new("registry_agent_resource_refs"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.build(PostgresQueryBuilder);
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let reference = (
			row.get::<String>("reference_id")
				.map_err(reinhardt::core::exception::Error::from)?,
			row.get::<String>("reference_version")
				.map_err(reinhardt::core::exception::Error::from)?,
		);
		if let Some(reason) = blocked.get(&reference) {
			impact.agents.push(Affected {
				id: row
					.get("agent_id")
					.map_err(reinhardt::core::exception::Error::from)?,
				version: Some(
					row.get("agent_version")
						.map_err(reinhardt::core::exception::Error::from)?,
				),
				reason: reason.clone(),
			});
		}
	}
	impact
		.agents
		.sort_by(|a, b| (&a.id, &a.version, &a.reason).cmp(&(&b.id, &b.version, &b.reason)));
	impact
		.agents
		.dedup_by(|a, b| a.id == b.id && a.version == b.version && a.reason == b.reason);
	Ok(impact)
}

/// Durable Run event is both the Thread disclosure and the audit record. One
/// event per Run survives later policy changes; no request content is retained.
pub(crate) async fn record_plan_exemption(
	store: &Store,
	context: &Context,
	policy: DataProtection,
) -> Result<()> {
	let Some(run) = context.run else {
		return Err(Error::Forbidden);
	};
	let scope = store.run_tenancy(run).await?;
	let mut tx = store
		.control_pool
		.begin(&scope)
		.await?
		.into_executor()
		.await?;
	// Serialize concurrent calls of the same Run so the disclosure is recorded once.
	let (sql, values) = Query::select()
		.columns(["id", "task_id", "workspace_id", "home_node", "node"].map(Alias::new))
		.from(Alias::new("runs"))
		.and_where(Expr::col("id").eq(Expr::value(run)))
		.lock(reinhardt::query::LockType::Update)
		.build(PostgresQueryBuilder);
	let row = tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or_else(|| Error::NotFound("run".into()))?;
	let task: uuid::Uuid = row
		.get("task_id")
		.map_err(reinhardt::core::exception::Error::from)?;
	let workspace: uuid::Uuid = row
		.get("workspace_id")
		.map_err(reinhardt::core::exception::Error::from)?;
	let home: String = row
		.get("home_node")
		.map_err(reinhardt::core::exception::Error::from)?;
	let node: String = row
		.get("node")
		.map_err(reinhardt::core::exception::Error::from)?;
	let local = aidash_domain::nodes::is_home(&home, store.deployment.as_str(), &node);
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("events"))
		.and_where(Expr::col("kind").eq(Expr::value("run.data_protection_exemption")))
		.and_where(Expr::cust("data ->> 'run_id'").eq(Expr::value(run.to_string())))
		.limit(1)
		.build(PostgresQueryBuilder);
	if tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.is_none()
	{
		crate::apps::execution::models::event_records::append_run(
			tx.as_mut(),
			run,
			local.then_some(workspace),
			"run.data_protection_exemption",
			serde_json::json!({"run_id":run,"task_id":task,"exemption":"plan_connection","policy":policy}),
		)
		.await?;
	}
	tx.commit().await?;
	tracing::info!(%run, tenant = %context.tenant, level = ?policy.level, exemption = "plan_connection", "Data Protection exemption used");
	Ok(())
}
