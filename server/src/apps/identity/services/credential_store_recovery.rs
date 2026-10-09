//! Offline, count-only recovery after loss of Provider Credential Store Master Keys.
use super::super::{
	repositories::{credential_store::PostgresStore, provider_credentials::NativeRepository},
	serializers::provider_credentials::StoreConfig,
};
use crate::{Error, Result};
use aidash_application::provider_credentials::{Repository, Service};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Serialize)]
struct Counts {
	affected_tenants: usize,
	affected_provider_credentials: usize,
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
	let repository = Arc::new(NativeRepository {
		pool,
		node: config.node_id,
	});
	let affected: Vec<_> = repository
		.active()
		.await?
		.into_iter()
		.filter(|row| {
			row.pinned_version
				.as_ref()
				.is_some_and(|pin| lost_pins.contains(pin))
		})
		.collect();
	let counts = Counts {
		affected_tenants: affected
			.iter()
			.map(|row| &row.tenant)
			.collect::<BTreeSet<_>>()
			.len(),
		affected_provider_credentials: affected.len(),
		version_rows: inventory.versions.len(),
		key_registry_rows: inventory.keys.len(),
	};
	if execute {
		let service = Service {
			repository,
			store: store.clone(),
			validator: Arc::new(
				aidash_integrations::provider_credentials::OpenRouterKeyValidator {
					client: aidash_integrations::semantic::client()?,
				},
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
		store.remove_unavailable(&inventory).await?;
	}
	Ok(counts)
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
