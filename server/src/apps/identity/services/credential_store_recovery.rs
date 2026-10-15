//! Offline, count-only recovery after loss of Provider Credential Store Master Keys.
use super::super::{
	repositories::{
		credential_store::PostgresStore, plan_connections::NativeRepository as PlanRepository,
		provider_credentials::NativeRepository,
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
			Error::Invalid("Provider Credential Store recovery requires a PostgreSQL Store".into())
		})?;
	if !matches!(keys, StoreConfig::Postgres { .. }) {
		return Err(Error::Invalid(
			"Provider Credential Store recovery requires a PostgreSQL Store".into(),
		));
	}
	let fingerprint_key = settings.provider_credentials.load_fingerprint_key().await?;
	let (current, retired) = keys.load_postgres_keys().await?;
	let config = crate::config::Config::from_settings(&settings)?;
	// Do not migrate or initialize the node: even a dry run must leave the database unchanged.
	let database = reinhardt::db::backends::DatabaseConnection::connect_postgres_with_pool_size(
		&config.database_url,
		Some(8),
	)
	.await?;
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
		affected_tenants: affected
			.iter()
			.map(|row| &row.tenant)
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
				.revoke(
					&row.tenant,
					row.id,
					row.revision,
					"provider-credential-store-recovery",
				)
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
		for id in plans {
			plan_service
				.mark_unreadable(id, "provider-credential-store-recovery")
				.await?;
		}
		store.remove_unavailable(&inventory).await?;
	}
	Ok(counts)
}

/// Plan Connections pinned to a version whose Master Key is gone.
async fn unreadable_plan_connections(
	pool: &crate::database::native::Pool,
	lost_pins: &BTreeSet<String>,
) -> Result<Vec<uuid::Uuid>> {
	use super::super::models::plan_connections::PlanConnection;
	use reinhardt::db::orm::Model;
	let mut tx = pool.connection().begin().await?;
	let rows = PlanConnection::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	tx.commit().await?;
	Ok(rows
		.into_iter()
		.filter(|row| {
			row.pinned_version
				.as_ref()
				.is_some_and(|pin| lost_pins.contains(pin))
		})
		.map(|row| row.id)
		.collect())
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("provider-credential-store-recovery")
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
