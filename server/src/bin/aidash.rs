//! Reinhardt command entry point.
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
	reinhardt::commands::shell_runtime_hook();
	aidash_server::bootstrap::management::run(true).await;
}

#[cfg(target_arch = "wasm32")]
fn main() {}
