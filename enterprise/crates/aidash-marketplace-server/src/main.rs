use aidash_marketplace_server::{Config, Marketplace, migrations};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let database = std::env::var("AIDASH_MARKETPLACE_DATABASE_URL")?;
	if std::env::args().nth(1).as_deref() == Some("migrate") {
		migrations::migrate(&database).await?;
		return Ok(());
	}
	let config = Config::from_env()?;
	let service = Marketplace::connect(config, &database).await?;
	let address = std::env::var("AIDASH_MARKETPLACE_LISTEN")
		.unwrap_or_else(|_| "127.0.0.1:8082".into())
		.parse()?;
	reinhardt::server::HttpServer::new(service.handler())
		.with_max_body_size(12 * 1024 * 1024)
		.listen(address)
		.await?;
	Ok(())
}
