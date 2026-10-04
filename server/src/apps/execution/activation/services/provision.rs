//! Provision durable activation transport without loading application or DB settings.
use super::{Broker, Settings};
use async_trait::async_trait;
use reinhardt::commands::{BaseCommand, CommandContext, CommandError, CommandResult};

pub struct ProvisionActivation;
#[async_trait]
impl BaseCommand for ProvisionActivation {
	fn name(&self) -> &str {
		"activation-provision"
	}
	fn description(&self) -> &str {
		"Provision the node's JetStream activation stream and durable consumer"
	}
	async fn execute(&self, context: &CommandContext) -> CommandResult<()> {
		if !context.args.is_empty() {
			return Err(CommandError::InvalidArguments(
				"activation-provision accepts no arguments".into(),
			));
		}
		provision()
			.await
			.map_err(|error| CommandError::ExecutionError(error.to_string()))
	}
}
async fn provision() -> crate::Result<()> {
	let node = std::env::var("AIDASH_NODE_ID")
		.map_err(|_| crate::Error::Invalid("AIDASH_NODE_ID is required".into()))?;
	crate::config::validate_node_id(&node)?;
	let settings = Settings::from_env()?;
	let broker = Broker::provision(
		&std::env::var("AIDASH_ACTIVATION_NATS_URL")
			.or_else(|_| std::env::var("NATS_URL"))
			.unwrap_or_else(|_| "nats://127.0.0.1:4222".into()),
		&node,
		&settings,
	)
	.await?;
	println!(
		"{}",
		serde_json::json!({"stream":broker.stream_name,"subject":broker.subject,"consumer":"workers-v1","consumer_created":broker.consumer.as_ref().map(|c|c.cached_info().created.to_string())})
	);
	Ok(())
}
