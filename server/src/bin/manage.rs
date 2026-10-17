//! Reinhardt command entry point.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
	// `manage serve` and `manage worker` read pages too, and Page reading
	// starts its bounded PDF child from the running binary (#312), so this
	// binary must run that one job and exit as `aidash` does.
	if let Some(status) =
		aidash_integrations::research::pdf::child_main(&std::env::args().collect::<Vec<_>>())
	{
		std::process::exit(status);
	}
	run();
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn run() {
	reinhardt::commands::shell_runtime_hook();
	aidash_server::bootstrap::management::run(false).await;
}

#[cfg(target_arch = "wasm32")]
fn main() {}
