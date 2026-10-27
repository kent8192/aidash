//! Cloud-only composition. No private signing key or provider key is configured here.
use aidash_broker::{
	Broker, CloudLogging, Config,
	tenant_secret::{TenantSecretBroker, TenantSecretConfig},
};
use aidash_capability::PublicKeys;
use aidash_gcp::capability::{
	MetadataTokenSource, SecretManagerKeyMaterialSource, SecretManagerTenantSecretSource,
	connections::SecretManagerConnectionStore,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

fn env(name: &str) -> Result<String, &'static str> {
	std::env::var(name).map_err(|_| "missing broker configuration")
}
async fn shutdown_signal() {
	#[cfg(unix)]
	let terminate = async {
		let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
			.expect("unable to install broker SIGTERM handler");
		signal.recv().await;
	};
	#[cfg(not(unix))]
	let terminate = std::future::pending::<()>();
	tokio::select! {
		_ = tokio::signal::ctrl_c() => {},
		_ = terminate => {},
	}
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	tracing_subscriber::fmt().json().with_target(false).init();
	match std::env::var("AIDASH_BROKER_ROLE") {
		Ok(role) if role == "tenant-secrets" => tenant_secrets().await,
		Ok(role) if role.is_empty() => key_material().await,
		Ok(_) => Err("unknown broker role".into()),
		Err(std::env::VarError::NotPresent) => key_material().await,
		Err(_) => Err("invalid broker role".into()),
	}
}
/// The Key Material role. It is unchanged and remains the default when no role
/// is configured.
async fn key_material() -> Result<(), Box<dyn std::error::Error>> {
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
		requests_per_second: rate()?,
		burst: burst()?,
		inference_deadline: timeout()?,
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
	serve(broker.router()).await
}
/// The Tenant Secret role. It has its own prefix, audience, issuer map and
/// public keys, and shares no configuration with the Key Material role.
async fn tenant_secrets() -> Result<(), Box<dyn std::error::Error>> {
	let project = env("AIDASH_BYOK_PROJECT_NUMBER")?;
	let prefix = env("AIDASH_TENANT_SECRET_PREFIX")?;
	let audience = env("AIDASH_SECRET_CALL_AUDIENCE")?;
	let issuers: BTreeMap<String, String> =
		serde_json::from_str(&env("AIDASH_SECRET_CALL_ISSUERS")?)
			.map_err(|_| "invalid Tenant Secret issuer configuration")?;
	let keys: BTreeMap<String, String> =
		serde_json::from_str(&env("AIDASH_CAPABILITY_PUBLIC_KEYS")?)
			.map_err(|_| "invalid public key configuration")?;
	let config = TenantSecretConfig {
		byok_project_number: project.clone(),
		secret_prefix: prefix.clone(),
		audience,
		issuers,
		requests_per_second: rate()?,
		burst: burst()?,
		timeout: timeout()?,
	};
	let source = SecretManagerTenantSecretSource::new(
		&project,
		&prefix,
		Arc::new(MetadataTokenSource::new()?),
	)?;
	let connections = aidash_broker::connection::ConnectionBroker::new(Arc::new(
		SecretManagerConnectionStore::new(
			&project,
			&prefix,
			Arc::new(MetadataTokenSource::new()?),
		)?,
	))?;
	let broker = Arc::new(
		TenantSecretBroker::new(
			config,
			PublicKeys::from_pems(keys)?,
			Arc::new(source),
			Arc::new(CloudLogging),
		)?
		.with_connections(connections),
	);
	serve(broker.router()).await
}
fn rate() -> Result<f64, Box<dyn std::error::Error>> {
	Ok(std::env::var("AIDASH_BROKER_REQUESTS_PER_SECOND")
		.unwrap_or_else(|_| "5".into())
		.parse()?)
}
fn burst() -> Result<u32, Box<dyn std::error::Error>> {
	Ok(std::env::var("AIDASH_BROKER_BURST")
		.unwrap_or_else(|_| "20".into())
		.parse()?)
}
fn timeout() -> Result<Duration, Box<dyn std::error::Error>> {
	Ok(Duration::from_secs(
		std::env::var("AIDASH_BROKER_TIMEOUT_SECS")
			.unwrap_or_else(|_| "3600".into())
			.parse()?,
	))
}
async fn serve(router: axum::Router) -> Result<(), Box<dyn std::error::Error>> {
	let port: u16 = std::env::var("PORT")
		.unwrap_or_else(|_| "8080".into())
		.parse()?;
	let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await?;
	axum::serve(listener, router)
		.with_graceful_shutdown(shutdown_signal())
		.await?;
	Ok(())
}

#[cfg(all(test, unix))]
mod tests {
	use super::*;
	use tokio::io::{AsyncBufReadExt, BufReader};

	#[tokio::test]
	async fn shutdown_handles_sigterm_and_sigint_in_an_isolated_process() {
		const CHILD: &str = "AIDASH_BROKER_SIGNAL_TEST_CHILD";
		if std::env::var_os(CHILD).is_some() {
			let signal = shutdown_signal();
			tokio::pin!(signal);
			assert!(futures_util::poll!(&mut signal).is_pending());
			println!("broker-signal-ready");
			signal.await;
			return;
		}
		for signal in ["-TERM", "-INT"] {
			let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
				.args([
					"--exact",
					"tests::shutdown_handles_sigterm_and_sigint_in_an_isolated_process",
					"--nocapture",
				])
				.env(CHILD, "1")
				.stdout(std::process::Stdio::piped())
				.kill_on_drop(true)
				.spawn()
				.unwrap();
			let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
			tokio::time::timeout(Duration::from_secs(10), async {
				while let Some(line) = lines.next_line().await.unwrap() {
					if line.contains("broker-signal-ready") {
						return;
					}
				}
				panic!("signal helper exited before installing handlers");
			})
			.await
			.unwrap();
			assert!(
				tokio::process::Command::new("kill")
					.args([signal, &child.id().unwrap().to_string()])
					.status()
					.await
					.unwrap()
					.success()
			);
			assert!(
				tokio::time::timeout(Duration::from_secs(10), child.wait())
					.await
					.unwrap()
					.unwrap()
					.success()
			);
		}
	}
}
