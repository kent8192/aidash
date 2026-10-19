//! Offline, count-only recovery after loss of Secret Store Master Keys.
use super::super::{
	repositories::{
		plan_connections::NativeRepository as PlanRepository,
		provider_credentials::NativeRepository, secret_store::PostgresStore,
	},
	serializers::provider_credentials::StoreConfig,
};
use crate::{Error, Result};
use aidash_application::provider_credentials::{RECONCILIATION_BATCH_SIZE, Repository, Service};
use aidash_domain::provider_credentials::State;
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Serialize)]
struct Counts {
	affected_tenants: usize,
	affected_provider_credentials: usize,
	/// Plan Connections whose pinned Plan Tokens become unreadable. Their
	/// owners must connect again; the grant cannot be revoked at OpenAI.
	affected_plan_connections: usize,
	version_rows: usize,
	key_registry_rows: usize,
}

async fn recover(execute: bool) -> Result<Counts> {
	let settings = crate::config::startup::load_settings()?;
	let keys = settings
		.provider_credentials
		.store
		.as_ref()
		.ok_or_else(|| {
			Error::Invalid("Secret Store recovery requires a PostgreSQL Store".into())
		})?;
	if !matches!(keys, StoreConfig::Postgres { .. }) {
		return Err(Error::Invalid(
			"Secret Store recovery requires a PostgreSQL Store".into(),
		));
	}
	let fingerprint_key = settings.provider_credentials.load_fingerprint_key().await?;
	let (current, retired) = keys.load_postgres_keys().await?;
	let config = crate::config::Config::from_settings(&settings)?;
	// Do not migrate or initialize the node: even a dry run must leave the database unchanged.
	let database = crate::database::runtime::connect(&config.database_url, 8).await?;
	let pool =
		crate::database::native::Pool::from(database.into_postgres().ok_or(Error::Forbidden)?);
	let store = Arc::new(PostgresStore::for_recovery(pool.clone(), current, retired).await?);
	let inventory = store.recovery_inventory().await?;
	let lost_pins: BTreeSet<_> = inventory
		.versions
		.iter()
		.map(|(resource, version)| format!("{resource}/versions/{version}"))
		.collect();
	let plans = unreadable_plan_connections(&pool, &lost_pins).await?;
	let plan_repository = Arc::new(PlanRepository {
		pool: pool.clone(),
		node: config.node_id.clone(),
	});
	let repository = Arc::new(NativeRepository {
		pool,
		node: config.node_id,
	});
	let mut affected = Vec::new();
	let mut cursor = None;
	loop {
		let page = repository
			.reconciliation_candidates(cursor, RECONCILIATION_BATCH_SIZE)
			.await?;
		cursor = page.last().map(|row| row.id);
		let complete = page.len() < RECONCILIATION_BATCH_SIZE;
		affected.extend(page.into_iter().filter(|row| {
			row.state == State::Active
				&& row
					.pinned_version
					.as_ref()
					.is_some_and(|pin| lost_pins.contains(pin))
		}));
		if complete {
			break;
		}
	}
	let counts = Counts {
		// A Tenant counts once, whether a Provider Credential, a Plan
		// Connection or both lost their key there.
		affected_tenants: affected
			.iter()
			.map(|row| row.tenant.as_str())
			.chain(plans.iter().map(|(_, tenant)| tenant.as_str()))
			.collect::<BTreeSet<_>>()
			.len(),
		affected_provider_credentials: affected.len(),
		affected_plan_connections: plans.len(),
		version_rows: inventory.versions.len(),
		key_registry_rows: inventory.keys.len(),
	};
	if execute {
		let client = aidash_integrations::semantic::client()?;
		let service = Service {
			repository,
			store: store.clone(),
			validator: Arc::new(
				aidash_integrations::provider_credentials::OpenRouterKeyValidator {
					client: client.clone(),
				},
			),
			exchange: Arc::new(
				aidash_integrations::provider_authorization::OpenRouterAuthorization::new(client),
			),
			fingerprint_key,
			max_per_tenant: settings.provider_credentials.max_per_tenant,
		};
		for row in affected {
			service
				.revoke(&row.tenant, row.id, row.revision, "secret-store-recovery")
				.await?;
		}
		let plan_service = aidash_application::plan_connections::Service {
			repository: plan_repository,
			store: store.clone(),
			reader: store.clone(),
			authorization: Arc::new(aidash_integrations::chatgpt::ChatgptAuthorization::new(
				aidash_integrations::semantic::client()?,
			)),
		};
		for (id, _) in plans {
			plan_service
				.mark_unreadable(id, "secret-store-recovery")
				.await?;
		}
		store.remove_unavailable(&inventory).await?;
	}
	Ok(counts)
}

/// Plan Connections pinned to a version whose Master Key is gone, scanned in
/// bounded pages by id like the Provider Credentials above: only matching
/// ids are kept, and no transaction outlives one page.
async fn unreadable_plan_connections(
	pool: &crate::database::native::Pool,
	lost_pins: &BTreeSet<String>,
) -> Result<Vec<(uuid::Uuid, String)>> {
	use reinhardt::db::orm::{QueryRow, execution::convert_values};
	use reinhardt::query::{
		Alias, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let mut found = Vec::new();
	let mut after: Option<uuid::Uuid> = None;
	loop {
		let mut query = Query::select();
		query
			.columns([
				Alias::new("id"),
				Alias::new("tenant"),
				Alias::new("pinned_version"),
			])
			.from(Alias::new("plan_connections"))
			.and_where(Expr::col(Alias::new("pinned_version")).is_not_null())
			.order_by(Alias::new("id"), Order::Asc)
			.limit(RECONCILIATION_BATCH_SIZE as u64);
		if let Some(after) = after {
			query.and_where(Expr::col(Alias::new("id")).gt(Expr::value(after)));
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		// Recovery reconciles every Tenant's pinned Plan Tokens.
		let scope = aidash_application::tenancy::TenancyScope::Deployment(
			aidash_application::tenancy::DeploymentScopeReason::SecretStoreRecovery,
		);
		let mut tx = pool.begin(&scope).await?.into_executor()?;
		let rows = tx.fetch_all(&sql, convert_values(values)).await?;
		tx.commit().await?;
		let page = rows
			.into_iter()
			.map(|row| {
				let data = QueryRow::from_backend_row(row).data;
				let id = data["id"]
					.as_str()
					.and_then(|id| id.parse::<uuid::Uuid>().ok())
					.ok_or_else(|| Error::Invalid("unreadable Plan Connection id".into()))?;
				let tenant = data["tenant"]
					.as_str()
					.ok_or_else(|| Error::Invalid("unreadable Plan Connection Tenant".into()))?
					.to_owned();
				Ok((
					id,
					tenant,
					data["pinned_version"].as_str().map(str::to_owned),
				))
			})
			.collect::<Result<Vec<_>>>()?;
		after = page.last().map(|(id, _, _)| *id);
		let complete = page.len() < RECONCILIATION_BATCH_SIZE;
		found.extend(
			page.into_iter()
				.filter(|(_, _, pin)| pin.as_ref().is_some_and(|pin| lost_pins.contains(pin)))
				.map(|(id, tenant, _)| (id, tenant)),
		);
		if complete {
			return Ok(found);
		}
	}
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("secret-store-recovery")
			.about("Report lost-key recovery counts; stop server and worker before using --execute")
			.arg(
				clap::Arg::new("execute")
					.long("execute")
					.action(clap::ArgAction::SetTrue),
			)
	}
	fn requirements(
		&self,
		_: &clap::ArgMatches,
	) -> Vec<reinhardt::commands::CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &reinhardt::commands::CapabilityContext,
	) -> reinhardt::commands::CommandResult<()> {
		let result: Result<()> = async {
			println!(
				"{}",
				serde_json::to_string(&recover(matches.get_flag("execute")).await?)?
			);
			Ok(())
		}
		.await;
		result.map_err(|error| reinhardt::commands::CommandError::ExecutionError(error.to_string()))
	}
}
