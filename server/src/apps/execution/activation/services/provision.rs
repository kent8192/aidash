//! Provision activation using the runtime's composed node and broker settings.
use super::{Broker, Settings};
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
	SettingsView,
};
use reinhardt::conf::settings::{builder::BuildError, scoped::ScopedSettings};

struct ActivationTarget {
	node_id: String,
	nats_url: String,
}
impl SettingsView for ActivationTarget {
	const NAME: &'static str = "activation target";
	fn resolve(settings: &ScopedSettings, alias: Option<&str>) -> Result<Self, BuildError> {
		if alias.is_some() {
			return Err(BuildError::Deserialization(
				"activation target has no aliases".into(),
			));
		}
		let node_id = settings.require_path::<String>(&["node", "node_id"])?;
		crate::config::validate_node_id(&node_id).map_err(|_| {
			BuildError::Deserialization("node.node_id must be a valid Aidash identity".into())
		})?;
		let nats_url = settings
			.optional_path::<String>(&["node", "nats_url"])?
			.unwrap_or_else(|| "nats://127.0.0.1:4222".into());
		let url = reqwest::Url::parse(&nats_url).map_err(|_| {
			BuildError::Deserialization("node.nats_url must identify a NATS server".into())
		})?;
		if !matches!(url.scheme(), "nats" | "tls" | "ws" | "wss") || url.host_str().is_none() {
			return Err(BuildError::Deserialization(
				"node.nats_url must identify a NATS server".into(),
			));
		}
		Ok(Self { node_id, nats_url })
	}
}

pub struct ProvisionActivation;
#[async_trait]
impl CapabilityCommand for ProvisionActivation {
	fn cli(&self) -> clap::Command {
		clap::Command::new("activation-provision")
			.about("Provision the node's JetStream activation stream and durable consumer")
	}
	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		vec![CapabilityRequirement::settings::<ActivationTarget>(None)]
	}
	async fn execute(
		&self,
		_: &clap::ArgMatches,
		context: &CapabilityContext,
	) -> CommandResult<()> {
		let target = context.settings::<ActivationTarget>(None)?;
		provision(&target)
			.await
			.map_err(|error| CommandError::ExecutionError(error.to_string()))
	}
}
async fn provision(target: &ActivationTarget) -> crate::Result<()> {
	let settings = Settings::from_env()?;
	// Match bootstrap's explicit dedicated-activation connection override.
	let url =
		std::env::var("AIDASH_ACTIVATION_NATS_URL").unwrap_or_else(|_| target.nats_url.clone());
	let broker = Broker::provision(&url, &target.node_id, &settings).await?;
	println!(
		"{}",
		serde_json::json!({"stream":broker.stream_name,"subject":broker.subject,"consumer":"workers-v1","consumer_created":broker.consumer.as_ref().map(|c|c.cached_info().created.to_string())})
	);
	Ok(())
}
