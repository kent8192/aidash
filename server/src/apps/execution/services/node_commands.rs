//! Deployment roles and static utilities registered with Reinhardt's command driver.
use crate::{Error, bootstrap, config::startup::load_settings};
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
};

#[derive(Clone, Copy)]
enum Role {
	Serve,
	Server,
	Worker,
}
struct RunNode(Role);

#[async_trait]
impl CapabilityCommand for RunNode {
	fn cli(&self) -> clap::Command {
		let (name, description) = match self.0 {
			Role::Serve => ("serve", "Run HTTP and background workers"),
			Role::Server => ("server", "Run the HTTP server without executing agent work"),
			Role::Worker => (
				"worker",
				"Run background workers without a management HTTP listener",
			),
		};
		clap::Command::new(name).about(description)
	}
	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(&self, _: &clap::ArgMatches, _: &CapabilityContext) -> CommandResult<()> {
		async {
			let _ = tracing_subscriber::fmt()
				.with_env_filter(
					tracing_subscriber::EnvFilter::try_from_default_env()
						.unwrap_or_else(|_| "aidash=info".into()),
				)
				.try_init();
			let settings = load_settings()?;
			bootstrap::migrate(&settings).await?;
			match self.0 {
				Role::Worker => super::worker::run(None).await,
				Role::Serve => bootstrap::serve(settings, true).await,
				Role::Server => bootstrap::serve(settings, false).await,
			}
		}
		.await
		.map_err(command_error)
	}
}

struct OpenApi;
#[async_trait]
impl CapabilityCommand for OpenApi {
	fn cli(&self) -> clap::Command {
		clap::Command::new("openapi").about("Export the public OpenAPI contract")
	}
	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(&self, _: &clap::ArgMatches, _: &CapabilityContext) -> CommandResult<()> {
		use reinhardt::commands::BaseCommand;
		BaseCommand::execute(&super::schema::ApiContract, &Default::default()).await
	}
}

struct CapabilityProfile;
#[async_trait]
impl CapabilityCommand for CapabilityProfile {
	fn cli(&self) -> clap::Command {
		clap::Command::new("capability-profile").about("Export the default capability profile")
	}
	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(&self, _: &clap::ArgMatches, _: &CapabilityContext) -> CommandResult<()> {
		println!(
			"{}",
			serde_json::to_string_pretty(&crate::capabilities::Profile::default())
				.map_err(Error::from)
				.map_err(command_error)?
		);
		Ok(())
	}
}

fn command_error(error: Error) -> CommandError {
	CommandError::ExecutionError(error.to_string())
}

pub fn commands() -> Vec<Box<dyn CapabilityCommand>> {
	vec![
		Box::new(RunNode(Role::Serve)),
		Box::new(RunNode(Role::Server)),
		Box::new(RunNode(Role::Worker)),
		Box::new(OpenApi),
		Box::new(CapabilityProfile),
		Box::new(crate::activation::services::provision::ProvisionActivation),
	]
}
