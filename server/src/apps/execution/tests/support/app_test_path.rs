// One shared upward traversal anchors cross-application integration-test imports.
macro_rules! app_test_path {
	($path:literal) => {
		concat!(
			env!("CARGO_MANIFEST_DIR"),
			"/src/apps/execution/tests/../../",
			$path
		)
	};
}
