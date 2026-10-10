//! Cloud-only composition. No private signing key or provider key is configured here.
use aidash_broker::{Broker, CloudLogging, Config};
use aidash_capability::PublicKeys;
use aidash_integrations::capability::{MetadataTokenSource, SecretManagerKeyMaterialSource};
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
