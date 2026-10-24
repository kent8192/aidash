//! The HTTP and management-command representations of the public API contract.
use crate::{Result, config::openapi::openapi};
use async_trait::async_trait;
use reinhardt::commands::{BaseCommand, CommandContext, CommandError, CommandResult};
use reinhardt::commands::{CapabilityCommand, CapabilityContext, CapabilityRequirement};
use reinhardt::injectable;
use reinhardt::rest::openapi::OpenApiSchema;
use std::io::{self, Write};

#[derive(Clone)]
pub struct ApiContract;

#[async_trait]
impl CapabilityCommand for ApiContract {
	fn cli(&self) -> clap::Command {
		clap::Command::new("exportopenapi").about(BaseCommand::description(self).to_owned())
	}

	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}

	async fn execute(&self, _: &clap::ArgMatches, _: &CapabilityContext) -> CommandResult<()> {
		BaseCommand::execute(self, &CommandContext::default()).await
	}
}

#[injectable(scope = "singleton")]
pub async fn provide_contract() -> ApiContract {
	ApiContract
}

impl ApiContract {
	pub fn document(&self) -> Result<OpenApiSchema> {
		openapi()
	}
}

#[async_trait]
impl BaseCommand for ApiContract {
	fn name(&self) -> &str {
		"exportopenapi"
	}
	fn description(&self) -> &str {
		"Write Aidash's public OpenAPI contract to stdout without runtime configuration"
	}
	fn requires_system_checks(&self) -> bool {
		false
	}
	async fn execute(&self, _: &CommandContext) -> CommandResult<()> {
		let document = self
			.document()
			.map_err(|error| CommandError::ExecutionError(error.to_string()))?;
		let mut output = io::stdout().lock();
		serde_json::to_writer_pretty(&mut output, &document)
			.map_err(|error| CommandError::ExecutionError(error.to_string()))?;
		output.write_all(b"\n")?;
		Ok(())
	}
}
