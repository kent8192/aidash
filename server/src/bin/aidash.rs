//! Compatibility entry point for existing node and worker deployments.
use aidash_server::{Error, Result, activation, config, config::startup::load_settings};

#[tokio::main]
async fn main() -> Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| "aidash_server=info".into()),
		)
		.init();
	let mode = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
	match mode.as_str() {
		"capability-profile" => {
			println!(
				"{}",
				serde_json::to_string_pretty(&aidash_server::capabilities::Profile::default())?
			);
			return Ok(());
		}
		"openapi" => {
			println!(
				"{}",
				serde_json::to_string_pretty(&config::openapi::openapi()?)?
			);
			return Ok(());
		}
		"activation-provision" => {
			let node = std::env::var("AIDASH_NODE_ID")
				.map_err(|_| Error::Invalid("AIDASH_NODE_ID is required".into()))?;
			config::validate_node_id(&node)?;
			let settings = activation::Settings::from_env()?;
			let broker = activation::Broker::provision(
				&std::env::var("AIDASH_ACTIVATION_NATS_URL")
					.or_else(|_| std::env::var("NATS_URL"))
					.unwrap_or_else(|_| "nats://127.0.0.1:4222".into()),
				&node,
				&settings,
			)
			.await?;
			println!(
				"{}",
				serde_json::json!({
					"stream": broker.stream_name, "subject": broker.subject,
					"consumer": "workers-v1",
					"consumer_created": broker.consumer.as_ref().map(|c| c.cached_info().created.to_string()),
				})
			);
			return Ok(());
		}
		"serve" | "server" | "worker" | "migrate" => {}
		_ => {
			return Err(Error::Invalid(
				"usage: aidash [serve|server|worker|migrate|activation-provision|openapi]".into(),
			));
		}
	}
	let settings = load_settings()?;
	aidash_server::bootstrap::migrate(&settings).await?;
	match mode.as_str() {
		"migrate" => Ok(()),
		"worker" => aidash_server::apps::execution::services::worker::run(None).await,
		_ => aidash_server::bootstrap::serve(settings, mode == "serve").await,
	}
}
