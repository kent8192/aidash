//! `manage issue-subject-credential --tenant <name> --subject <id> [--label <text>]`
//! issues a Subject Credential from the host (#484, #301).
//!
//! A Deployment Administrator holds no Tenant authority, so the first
//! credential of a Tenant in a headless setup comes from whoever operates the
//! host: the command opens the database with the Deployment's own settings and
//! never goes through HTTP or the shared `api_token`. The Host Access administrator is
//! trusted, so it may issue for any subject the Tenant's policy defines and
//! enables, including a user subject that a person's Mapping also covers. The
//! issuance is recorded exactly as the HTTP path records it: a credential row
//! naming its issuer (`--label`, default `host_operator`).
use super::Authorization;
use crate::apps::identity::serializers::identity::IssuedCredential;
use crate::config::Config;
use crate::database::native::Pool;
use crate::{Error, Result};

/// The issuer recorded when `--label` is not given.
pub const DEFAULT_LABEL: &str = "host_operator";
/// The lifetime of a credential issued without `--expires-in-seconds`.
pub const DEFAULT_LIFETIME_SECONDS: i64 = 3600;

/// Issues one Subject Credential of `tenant` for `subject` through the same
/// code the HTTP issuance path of a Deployment Administrator used. The token is
/// in the result exactly once; only its digest is stored.
pub async fn issue(
	pool: &Pool,
	tenant: &str,
	subject: &str,
	lifetime_seconds: i64,
	label: &str,
) -> Result<IssuedCredential> {
	Authorization { pool: pool.clone() }
		.issue_credential(tenant, subject, lifetime_seconds, label)
		.await
}

async fn run(tenant: &str, subject: &str, lifetime: i64, label: &str) -> Result<IssuedCredential> {
	let settings = crate::config::startup::load_settings()?;
	crate::bootstrap::require_migrated(&settings).await?;
	let config = Config::from_settings(&settings)?;
	let database = crate::database::runtime::connect(&config.database_url, 4).await?;
	let pool = Pool::from(database.into_postgres().ok_or(Error::Forbidden)?);
	let issued = issue(&pool, tenant, subject, lifetime, label).await;
	pool.close().await;
	issued
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("issue-subject-credential")
			.about("Issue a Subject Credential from the host and print it once as JSON")
			.arg(
				clap::Arg::new("tenant")
					.long("tenant")
					.required(true)
					.value_name("TENANT"),
			)
			.arg(
				clap::Arg::new("subject")
					.long("subject")
					.required(true)
					.value_name("SUBJECT"),
			)
			.arg(
				clap::Arg::new("label")
					.long("label")
					.value_name("TEXT")
					.help("The issuer recorded with the credential (identifier syntax)"),
			)
			.arg(
				clap::Arg::new("expires_in_seconds")
					.long("expires-in-seconds")
					.value_name("SECONDS")
					.value_parser(clap::value_parser!(i64)),
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
					Error::Invalid(format!("issue-subject-credential requires --{name}"))
				})
			};
			let label = matches
				.get_one::<String>("label")
				.map_or(DEFAULT_LABEL, String::as_str);
			let lifetime = matches
				.get_one::<i64>("expires_in_seconds")
				.copied()
				.unwrap_or(DEFAULT_LIFETIME_SECONDS);
			let issued = run(required("tenant")?, required("subject")?, lifetime, label).await?;
			println!("{}", serde_json::to_string(&issued)?);
			Ok(())
		}
		.await;
		result.map_err(|error| reinhardt::commands::CommandError::ExecutionError(error.to_string()))
	}
}
