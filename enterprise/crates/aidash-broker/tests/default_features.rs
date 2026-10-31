//! The loopback upstream origin is a test-only seam. The release binary must
//! never compile it: the default feature set is empty, and `test-support` is
//! enabled only by test integration targets.

#[cfg(not(feature = "test-support"))]
#[test]
fn the_default_build_compiles_no_upstream_origin_override() {
	const {
		assert!(
			!cfg!(feature = "test-support"),
			"the default build must not enable the test-only upstream origin override"
		);
	}
}

#[test]
fn the_upstream_origin_override_is_not_a_default_feature() {
	let manifest = include_str!("../Cargo.toml");
	let features = manifest
		.split_once("[features]")
		.expect("the broker manifest declares its features")
		.1;
	assert!(
		features.contains("default = []"),
		"the broker's default feature set must stay empty"
	);
	assert!(
		features.contains("test-support = []"),
		"the upstream origin override stays behind `test-support`"
	);
}
