//! Export the disabled-by-default runtime profile for operator configuration.
use async_trait::async_trait;
use reinhardt::commands::{BaseCommand, CommandContext, CommandError, CommandResult};

pub struct CapabilityProfile;
#[async_trait]
impl BaseCommand for CapabilityProfile {
	fn name(&self) -> &str {
		"capability-profile"
	}
	fn description(&self) -> &str {
		"Print the default capability profile as JSON"
	}
	async fn execute(&self, context: &CommandContext) -> CommandResult<()> {
		if !context.args.is_empty() {
			return Err(CommandError::InvalidArguments(
				"capability-profile accepts no arguments".into(),
			));
		}
		println!(
			"{}",
			serde_json::to_string_pretty(&super::Profile::default())
				.map_err(|error| CommandError::ExecutionError(error.to_string()))?
		);
		Ok(())
	}
}
