//! Reinhardt command entry point of the Community build.
//!
//! `manage serve` and `manage worker` read pages too, and Page reading starts
//! its bounded PDF child from the running binary (#312), so this binary runs
//! that one job and exits as `aidash` does.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
	use aidash_server::bootstrap::{build::Build, management};
	management::main(&Build::COMMUNITY, false);
}

#[cfg(target_arch = "wasm32")]
fn main() {}
