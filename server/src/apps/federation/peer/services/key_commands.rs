//! `manage deployment-key generate|show`: the Deployment Key of ADR 0064.
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
			.about("Create or show the Deployment Key that signs Federation tokens")
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
				clap::Command::new("show").about("Print the key ID and the public key of the Deployment Key"),
			)
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
				_ => return Err(Error::Invalid("name generate or show".into())),
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
