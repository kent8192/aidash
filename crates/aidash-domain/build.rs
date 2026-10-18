//! Exposes the workspace root as `AIDASH_WORKSPACE_DIR`, so `include_bytes!`
//! paths can be written from the repository root.
use std::env;
use std::path::PathBuf;

fn main() {
	let manifest_dir =
		PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
	// The crate lives at `crates/aidash-domain` below the workspace root.
	let workspace_dir = manifest_dir
		.ancestors()
		.nth(2)
		.expect("aidash-domain sits two directories below the workspace root");
	println!(
		"cargo:rustc-env=AIDASH_WORKSPACE_DIR={}",
		workspace_dir.display()
	);
	println!("cargo:rerun-if-changed=build.rs");
}
