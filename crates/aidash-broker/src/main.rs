//! Cloud-only composition. No private signing key or provider key is configured here.
use aidash_broker::{Broker, CloudLogging, Config};
use aidash_capability::PublicKeys;
use aidash_integrations::capability::{MetadataTokenSource, SecretManagerKeyMaterialSource};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

fn env(name: &str) -> Result<String, &'static str> {
	std::env::var(name).map_err(|_| "missing broker configuration")
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	tracing_subscriber::fmt().json().with_target(false).init();
	let project = env("AIDASH_BYOK_PROJECT_NUMBER")?;
	let prefix = env("AIDASH_PROVIDER_CREDENTIAL_PREFIX")?;
	let keys: BTreeMap<String, String> =
		serde_json::from_str(&env("AIDASH_CAPABILITY_PUBLIC_KEYS")?)
			.map_err(|_| "invalid public key configuration")?;
	let config = Config {
		issuer: env("AIDASH_CAPABILITY_ISSUER")?,
		audience: env("AIDASH_CAPABILITY_AUDIENCE")?,
		byok_project_number: project.clone(),
		secret_prefix: prefix.clone(),
		requests_per_second: std::env::var("AIDASH_BROKER_REQUESTS_PER_SECOND")
			.unwrap_or_else(|_| "5".into())
			.parse()?,
		burst: std::env::var("AIDASH_BROKER_BURST")
			.unwrap_or_else(|_| "20".into())
			.parse()?,
		inference_deadline: Duration::from_secs(
			std::env::var("AIDASH_BROKER_TIMEOUT_SECS")
				.unwrap_or_else(|_| "3600".into())
				.parse()?,
		),
	};
	let source = SecretManagerKeyMaterialSource::new(
		&project,
		&prefix,
		Arc::new(MetadataTokenSource::new()?),
	)?;
	let broker = Arc::new(Broker::new(
		config,
		PublicKeys::from_pems(keys)?,
		Arc::new(source),
		Arc::new(CloudLogging),
	)?);
	let port: u16 = std::env::var("PORT")
		.unwrap_or_else(|_| "8080".into())
		.parse()?;
	let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await?;
	axum::serve(listener, broker.router())
		.with_graceful_shutdown(async {
			let _ = tokio::signal::ctrl_c().await;
		})
		.await?;
	Ok(())
}
