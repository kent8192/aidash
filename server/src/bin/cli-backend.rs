//! Separate, loopback-only inference process; never loads Aidash server secrets.
#[path = "../cli_backend.rs"]
mod cli_backend;

#[tokio::main]
async fn main() {
	if let Err(error) = cli_backend::main().await {
		eprintln!("cli-backend: {error}");
		std::process::exit(1);
	}
}
