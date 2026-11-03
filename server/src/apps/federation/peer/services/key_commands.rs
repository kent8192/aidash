//! `manage deployment-key`: initial creation and durable normal rollover (ADR 0084).
use crate::apps::operations::serializers::deployment::{
	DeploymentKeySettings, DeploymentKeySource,
};
use crate::{Error, Result, bootstrap};
use aidash_application::federation::key::DeploymentKey;
use aidash_integrations::federation::key::FileDeploymentKey;
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
};
use std::path::{Path, PathBuf};

pub struct Command;

#[async_trait]
impl CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("deployment-key")
			.about("Create, inspect or roll over the Deployment Key that signs Federation tokens")
			.subcommand_required(true)
			.arg_required_else_help(true)
			.subcommand(
				clap::Command::new("generate")
					.about(
						"Write a new Ed25519 key to the [deployment.key] file, readable by its owner only",
					)
					.arg(
						clap::Arg::new("force")
							.long("force")
							.action(clap::ArgAction::SetTrue)
							.help(
								"Replace an existing key. Every Node Registration that pins the old key ID must then be registered again",
							),
					),
			)
			.subcommand(
				clap::Command::new("show").about("Print the configured initial source (use status for scheduled keys)"),
			)
            .subcommand(clap::Command::new("status")
                .about("Read the authoritative current key and rollover schedule from the database"))
            .subcommand(clap::Command::new("rollover")
                .about("Publish a successor: activate after 24 hours, retire the predecessor seven days later")
                .arg(clap::Arg::new("file").long("file").value_name("PATH").conflicts_with("kms").required_unless_present("kms"))
                .arg(clap::Arg::new("kms").long("kms").value_name("VERSION").conflicts_with("file").required_unless_present("file"))
                .arg(clap::Arg::new("expected-current").long("expected-current").value_name("KEY_ID").required(true)
                    .help("Current key ID from status; refuse a changed identity")))
	}
	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &CapabilityContext,
	) -> CommandResult<()> {
		let result = async {
			let (settings, base_dir) = load()?;
			let report = match matches.subcommand() {
				Some(("generate", arguments)) => {
					generate(settings.as_ref(), &base_dir, arguments.get_flag("force"))?
				}
				Some(("show", _)) => show(settings.as_ref(), &base_dir).await?,
				Some(("status", _)) => operate(None).await?,
				Some(("rollover", arguments)) => operate(Some(arguments)).await?,
				_ => {
					return Err(Error::Invalid(
						"name generate, show, status or rollover".into(),
					));
				}
			};
			print!("{report}");
			Ok(())
		}
		.await;
		result.map_err(|error: Error| CommandError::ExecutionError(error.to_string()))
	}
}

/// Only `[deployment.key]` and the base directory are read, so the command
/// works before the rest of the settings, or the database, exist.
fn load() -> Result<(Option<DeploymentKeySettings>, PathBuf)> {
	let invalid =
		|error: reinhardt::conf::settings::builder::BuildError| Error::Invalid(error.to_string());
	let settings = crate::config::settings::get_scoped_settings().map_err(invalid)?;
	bootstrap::build::current()
		.check(&settings)
		.map_err(invalid)?;
	Ok((
		settings
			.optional_path::<DeploymentKeySettings>(&["deployment", "key"])
			.map_err(invalid)?,
		settings
			.require_path::<PathBuf>(&["core", "base_dir"])
			.map_err(invalid)?,
	))
}

pub(crate) fn generate(
	settings: Option<&DeploymentKeySettings>,
	base_dir: &Path,
	replace: bool,
) -> Result<String> {
	let Some(settings) = settings else {
		return Err(Error::Invalid(
			"[deployment.key] file is not set; name the file to write the key to".into(),
		));
	};
	let DeploymentKeySource::File(file) = settings.source().map_err(Error::Invalid)? else {
		return Err(Error::Invalid(
			"the Deployment Key is in Cloud KMS; generate writes only a file".into(),
		));
	};
	let path = base_dir.join(file);
	let key = FileDeploymentKey::generate(&path, replace)?;
	Ok(format!(
		"Wrote a new Deployment Key to {}\n{}",
		path.display(),
		describe(&key)
	))
}

pub(crate) async fn show(
	settings: Option<&DeploymentKeySettings>,
	base_dir: &Path,
) -> Result<String> {
	let key = bootstrap::configure_deployment_key(settings, base_dir)
		.await?
		.ok_or_else(|| Error::Invalid("this Deployment has no [deployment.key]".into()))?;
	Ok(describe(key.as_ref()))
}

/// The key ID is what a Tenant Administrator types to pin this Deployment.
fn describe(key: &dyn DeploymentKey) -> String {
	format!(
		"Key ID: {}\n{}\n",
		key.key_id(),
		key.public_key_pem().trim_end()
	)
}

#[cfg(test)]
#[path = "../tests/key_commands.rs"]
mod tests;

/// Scheduled operations open only the database, never initialize Runs, migrate
/// schemas or change KMS resources. Initial generation retains its early path.
async fn operate(arguments: Option<&clap::ArgMatches>) -> Result<String> {
	use aidash_application::federation::key::{DeploymentKeySource, DeploymentKeys};
	let settings = crate::config::startup::load_settings()?;
	let config = crate::config::Config::from_settings(&settings)?;
	let database = crate::database::runtime::connect_maintenance(&config.database_url, 2).await?;
	let pool = crate::database::native::Pool::from(
		database
			.into_postgres()
			.ok_or_else(|| Error::Invalid("Aidash requires PostgreSQL".into()))?,
	);
	let keys = bootstrap::configure_deployment_keys(
		settings.deployment.key.as_ref(),
		&settings.core.base_dir,
		config.deployment,
		pool,
	)
	.await?
	.ok_or_else(|| Error::Invalid("this Deployment has no [deployment.key]".into()))?;
	let now = chrono::Utc::now().timestamp().max(0) as u64;
	let public = if let Some(arguments) = arguments {
		let expected = aidash_domain::nodes::DeploymentKeyId::parse(
			arguments
				.get_one::<String>("expected-current")
				.expect("required by clap"),
		)?;
		let source = match arguments.get_one::<String>("file") {
			Some(file) => DeploymentKeySource::File(
				std::fs::canonicalize(settings.core.base_dir.join(file)).map_err(|_| {
					Error::Invalid("successor Deployment Key file is unavailable".into())
				})?,
			),
			None => DeploymentKeySource::Kms(
				arguments
					.get_one::<String>("kms")
					.expect("required by clap")
					.clone(),
			),
		};
		keys.rollover(&expected, source, now).await?
	} else {
		keys.publication(now).await?
	};
	let mut report = format!(
		"Key ID: {}\n{}\n",
		public.current.id,
		public.current.public_key_pem.trim_end()
	);
	if let Some(schedule) = public.schedule {
		report.push_str(&format!(
			"Published at: {}\nActivates at: {}\nPredecessor retires at: {}\n",
			schedule.published_at, schedule.activates_at, schedule.retires_at
		));
	}
	if let Some(next) = public.successor {
		report.push_str(&format!("Successor key ID: {}\n", next.id));
	}
	if let Some(old) = public.retiring {
		report.push_str(&format!("Retiring key ID: {}\n", old.id));
	}
	Ok(report)
}
