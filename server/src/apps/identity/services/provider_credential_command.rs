//! `manage store-provider-credential --tenant <name> --provider <id>` stores a
//! provider key from the host into one Tenant's Secret Store (#892).
//!
//! A Self-hosted Deployment's host already reads its Secret Store (ADR 0008),
//! so Host Access may hand it Key Material directly, for example on a node
//! whose browser cannot complete Provider Authorization. The key is read from
//! standard input, never from an argument or an environment variable, so it
//! stays out of shell history and `ps`. The Provider Credential records its
//! key as stored through Host Access, and is bound when its provider has no
//! usable binding, as a connection is.
use super::super::{
	repositories::{provider_credentials::NativeRepository, secret_store::PostgresStore},
	serializers::provider_credentials::StoreConfig,
};
use crate::config::Config;
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::{
	provider_authorization::Target,
	provider_credentials::{Service, Validated},
};
use aidash_domain::{
	offering::Offering,
	provider_credentials::{Binding, KeyOrigin, Provider, State},
};
use secrecy::SecretString;
use std::{io::Read, sync::Arc};
use uuid::Uuid;

/// The actor recorded with every change this command makes.
pub const ACTOR: &str = "host_access";

/// What the command printed: the stored metadata and whether Runs use it.
#[derive(serde::Serialize)]
struct Stored {
	#[serde(flatten)]
	validated: Validated,
	bound: bool,
}

/// Binds `id` when `provider` has no binding to an active Provider
/// Credential; an active binding is never replaced.
async fn bind_if_unbound(
	service: &Service,
	tenant: &str,
	provider: Provider,
	id: Uuid,
) -> Result<bool> {
	let mut scope = service.repository.begin(tenant).await?;
	let binding: Option<Binding> = scope
		.bindings()
		.await?
		.into_iter()
		.find(|b| b.provider == provider);
	let usable = match binding.as_ref().and_then(|b| b.provider_credential_id) {
		Some(bound) => scope.get(bound).await?.state == State::Active,
		None => false,
	};
	scope.commit().await?;
	if usable {
		return Ok(binding.and_then(|b| b.provider_credential_id) == Some(id));
	}
	service
		.bind(
			tenant,
			provider,
			id,
			binding.map_or(0, |b| b.revision),
			ACTOR,
		)
		.await?;
	Ok(true)
}

async fn run(tenant: &str, provider: &str, target: Target, key: SecretString) -> Result<Stored> {
	if crate::bootstrap::build::offering() != Offering::SelfHosted {
		return Err(Error::Invalid(
			"only a Self-hosted Deployment stores provider keys from the host".into(),
		));
	}
	aidash_domain::policy::identifier(tenant)?;
	let provider = Provider::credential(provider)?;
	let settings = crate::config::startup::load_settings()?;
	let keys = settings
		.provider_credentials
		.store
		.as_ref()
		.filter(|store| matches!(store, StoreConfig::Postgres { .. }))
		.ok_or_else(|| {
			Error::Invalid("storing a provider key needs a PostgreSQL Secret Store".into())
		})?;
	crate::bootstrap::require_migrated(&settings).await?;
	let fingerprint_key = settings.provider_credentials.load_fingerprint_key().await?;
	let (current, retired) = keys.load_postgres_keys().await?;
	let config = Config::from_settings(&settings)?;
	let database = crate::database::runtime::connect_command(&config.database_url, 4).await?;
	let pool = Pool::from(database.into_postgres().ok_or(Error::Forbidden)?);
	let client = aidash_integrations::semantic::client()?;
	let service = Service {
		repository: Arc::new(NativeRepository {
			pool: pool.clone(),
			node: config.deployment.to_string(),
		}),
		store: Arc::new(PostgresStore::new(pool.clone(), current, retired).await?),
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
		supplied_keys: true,
	};
	let stored = async {
		let validated = service
			.supply(tenant, provider, target, key, KeyOrigin::HostAccess, ACTOR)
			.await?;
		let bound = match target {
			Target::Connect { id } => bind_if_unbound(&service, tenant, provider, id).await?,
			Target::Reconnect { id, .. } => {
				let mut scope = service.repository.begin(tenant).await?;
				let bound = scope
					.bindings()
					.await?
					.iter()
					.any(|b| b.provider == provider && b.provider_credential_id == Some(id));
				scope.commit().await?;
				bound
			}
		};
		Ok::<_, Error>(Stored { validated, bound })
	}
	.await;
	pool.close().await;
	stored
}

/// Reads one key from standard input, dropping a trailing newline.
fn read_key() -> Result<SecretString> {
	let mut key = String::new();
	std::io::stdin()
		.read_to_string(&mut key)
		.map_err(|error| Error::Invalid(format!("could not read the key from stdin: {error}")))?;
	let trimmed = key.trim_end_matches(['\r', '\n']).to_owned();
	Ok(SecretString::from(trimmed))
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("store-provider-credential")
			.about(
				"Store a provider key read from stdin as a Tenant's Provider Credential \
				 (Self-hosted only) and print its metadata as JSON",
			)
			.arg(
				clap::Arg::new("tenant")
					.long("tenant")
					.required(true)
					.value_name("TENANT"),
			)
			.arg(
				clap::Arg::new("provider")
					.long("provider")
					.required(true)
					.value_name("PROVIDER")
					.help(
						"A Provider Catalog entry served by Provider Credentials, such as openrouter",
					),
			)
			.arg(
				clap::Arg::new("replace")
					.long("replace")
					.value_name("PROVIDER_CREDENTIAL_ID")
					.value_parser(clap::value_parser!(Uuid))
					.requires("expected_revision")
					.help("Store the key as a new version of this Provider Credential"),
			)
			.arg(
				clap::Arg::new("expected_revision")
					.long("expected-revision")
					.value_name("REVISION")
					.value_parser(clap::value_parser!(i64))
					.requires("replace"),
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
			let required = |name: &str| {
				matches.get_one::<String>(name).ok_or_else(|| {
					Error::Invalid(format!("store-provider-credential requires --{name}"))
				})
			};
			let target = match (
				matches.get_one::<Uuid>("replace").copied(),
				matches.get_one::<i64>("expected_revision").copied(),
			) {
				(Some(id), Some(expected_revision)) => Target::Reconnect {
					id,
					expected_revision,
				},
				_ => Target::Connect { id: Uuid::now_v7() },
			};
			let key = read_key()?;
			let stored = run(required("tenant")?, required("provider")?, target, key).await?;
			println!("{}", serde_json::to_string(&stored)?);
			Ok(())
		}
		.await;
		result.map_err(|error| reinhardt::commands::CommandError::ExecutionError(error.to_string()))
	}
}
