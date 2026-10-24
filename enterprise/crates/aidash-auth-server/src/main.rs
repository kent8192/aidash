//! Standalone service entry point. Schema owners migrate explicitly before serve.
use aidash_auth_server::{Error, config::Config, service::Authentication};
use std::{path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() {
	tracing_subscriber::fmt()
		.with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
		.init();
	if let Err(error) = run().await {
		tracing::error!(error=%error,"authentication command failed");
		std::process::exit(1);
	}
}
async fn run() -> Result<(), Error> {
	let mut args = std::env::args().skip(1);
	let first = args.next();
	let (path, command) = if first.as_deref() == Some("--config") {
		(
			PathBuf::from(args.next().ok_or(Error::Configuration)?),
			args.next().unwrap_or_else(|| "serve".into()),
		)
	} else {
		(
			PathBuf::from(std::env::var_os("AIDASH_AUTH_CONFIG").ok_or(Error::Configuration)?),
			first.unwrap_or_else(|| "serve".into()),
		)
	};
	let mut config = Config::read(&path).map_err(|_| Error::Configuration)?;
	if let Some(path) = std::env::var_os("AIDASH_AUTH_ASSETS_DIR") {
		config.assets_dir = Some(path.into());
	}
	let store = Authentication::connect(config.clone()).await?;
	match command.as_str() {
		"migrate" => {
			if args.next().is_some() {
				return Err(Error::Invalid);
			}
			store.migrate().await?;
			Authentication::provision(&config, &store).await?;
		}
		"import-legacy" => {
			let path = PathBuf::from(args.next().ok_or(Error::Invalid)?);
			if args.next().is_some() {
				return Err(Error::Invalid);
			}
			let body = std::fs::read(path).map_err(|_| Error::Configuration)?;
			let identities: Vec<aidash_domain::accounts::ExternalIdentity> =
				serde_json::from_slice(&body).map_err(|_| Error::Invalid)?;
			store.import_legacy(&identities).await?;
		}
		"serve" => {
			if args.next().is_some()
				|| config
					.assets_dir
					.as_ref()
					.is_none_or(|p| !p.join("index.html").is_file())
			{
				return Err(Error::Configuration);
			}
			let listen = config.listen;
			let auth = Arc::new(Authentication::new(config, store).await?);
			let housekeeping = auth.clone();
			tokio::spawn(async move {
				let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
				loop {
					interval.tick().await;
					if housekeeping.store.purge_expired().await.is_err() {
						tracing::warn!("authentication ephemeral-state cleanup unavailable");
					}
				}
			});
			let http = reinhardt::server::serve(
				listen,
				Arc::new(aidash_auth_server::http::Http::new(auth.clone())),
			);
			let grpc = aidash_auth_server::grpc::serve(auth);
			tokio::select! { result = http => { result.map_err(|_|Error::Unavailable)?; }, result = grpc => { result?; } }
		}
		_ => return Err(Error::Invalid),
	}
	Ok(())
}
