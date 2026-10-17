//! Reinhardt command entry point.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
	// A bounded PDF child of Page reading runs one job and exits (#312). It
	// starts no runtime, so its address space limit is left for parsing.
	if let Some(status) =
		aidash_integrations::research::pdf::child_main(&std::env::args().collect::<Vec<_>>())
	{
		std::process::exit(status);
	}
	serve();
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn serve() {
	reinhardt::commands::shell_runtime_hook();
	aidash_server::bootstrap::management::run(true).await;
}

#[cfg(target_arch = "wasm32")]
fn main() {}
