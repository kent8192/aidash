//! Reinhardt command entry point of the Enterprise build.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
	aidash_server::bootstrap::management::main(&aidash_enterprise_server::BUILD, true);
}

#[cfg(target_arch = "wasm32")]
fn main() {}
