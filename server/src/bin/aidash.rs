//! Reinhardt command entry point of the Community build.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
	use aidash_server::bootstrap::{build::Build, management};
	management::main(&Build::COMMUNITY, true);
}

#[cfg(target_arch = "wasm32")]
fn main() {}
