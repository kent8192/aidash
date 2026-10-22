use super::*;
use crate::Error;
use rstest::rstest;

fn with_manifest(extra: Vec<PluginFile>) -> Vec<PluginFile> {
	let mut files = vec![manifest_file("demo")];
	files.extend(extra);
	files
}

fn padding(path: &str, len: usize) -> PluginFile {
	PluginFile {
		path: path.to_owned(),
		bytes: vec![b'x'; len],
	}
}

#[rstest]
#[case::empty("")]
#[case::empty_segment("a//b")]
#[case::absolute("/etc/passwd")]
#[case::trailing_slash("a/")]
#[case::current_directory("./a")]
#[case::dot_segment("a/./b")]
#[case::parent_segment("a/../b")]
#[case::leading_parent("../a")]
#[case::backslash("a\\b")]
#[case::newline("a\nb")]
#[case::nul("a\0b")]
#[case::too_long(&"a".repeat(241))]
fn invalid_paths_refuse_the_plugin(#[case] path: &str) {
	let files = with_manifest(vec![file(path, "x")]);
	assert_eq!(load(&files), Err(Rejection::InvalidPath(path.to_owned())));
}

#[rstest]
#[case::dot_prefixed(".npmrc")]
#[case::dot_directory("a/.b/c")]
#[case::longest(&"a".repeat(240))]
#[case::spaces_and_unicode("docs/guide 日本語.md")]
fn valid_paths_load(#[case] path: &str) {
	assert!(load(&with_manifest(vec![file(path, "x")])).is_ok());
}

#[test]
fn a_duplicate_path_refuses_the_plugin() {
	let files = with_manifest(vec![file("a.txt", "one"), file("a.txt", "two")]);
	assert_eq!(load(&files), Err(Rejection::InvalidPath("a.txt".into())));
}

#[test]
fn paths_are_checked_before_the_manifest_is_read() {
	let files = vec![file("a//b", "x")];
	assert_eq!(load(&files), Err(Rejection::InvalidPath("a//b".into())));
}

#[test]
fn five_hundred_twelve_files_load_and_one_more_does_not() {
	let numbered = |count: usize| {
		(0..count)
			.map(|n| file(&format!("f{n}.txt"), "x"))
			.collect::<Vec<_>>()
	};
	assert!(load(&with_manifest(numbered(511))).is_ok());
	assert_eq!(
		load(&with_manifest(numbered(512))),
		Err(Rejection::PackageLimit)
	);
}

#[test]
fn eight_mebibytes_load_and_one_more_byte_does_not() {
	let limit = 8 * 1024 * 1024;
	let manifest_len = manifest_file("demo").bytes.len();
	let at_limit = with_manifest(vec![padding("big.bin", limit - manifest_len)]);
	assert!(load(&at_limit).is_ok());
	let over = with_manifest(vec![padding("big.bin", limit - manifest_len + 1)]);
	assert_eq!(load(&over), Err(Rejection::PackageLimit));
}

#[test]
fn limits_are_checked_before_anything_is_parsed() {
	let files = vec![padding("big.bin", 8 * 1024 * 1024 + 1)];
	assert_eq!(load(&files), Err(Rejection::PackageLimit));
	let files: Vec<_> = (0..513).map(|n| file(&format!("f{n}"), "")).collect();
	assert_eq!(load(&files), Err(Rejection::PackageLimit));
}

#[test]
fn the_digest_follows_the_documented_construction() {
	let files = [file("dir/b.txt", "world"), file("a.txt", "hello")];
	assert_eq!(
		tree_digest(&files),
		"sha256:1c6529131cd8c1d5f43cb4cbcad8f7f7066f57b18ab704adf6456878a46e8140"
	);
	assert_eq!(
		tree_digest(&[]),
		"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
	);
}

#[test]
fn the_digest_ignores_input_order_and_sees_every_change() {
	let files = [file("a.txt", "hello"), file("dir/b.txt", "world")];
	let reversed = [files[1].clone(), files[0].clone()];
	assert_eq!(tree_digest(&files), tree_digest(&reversed));

	let one_byte = [file("a.txt", "hellp"), files[1].clone()];
	let renamed = [file("c.txt", "hello"), files[1].clone()];
	let extra = [files[0].clone(), files[1].clone(), file("e.txt", "")];
	let moved_boundary = [file("a.tx", "thello"), files[1].clone()];
	for changed in [&one_byte[..], &renamed[..], &extra[..], &moved_boundary[..]] {
		assert_ne!(tree_digest(&files), tree_digest(changed));
	}
}

#[test]
fn a_loaded_plugin_carries_the_digest_of_its_input() {
	let files = with_manifest(vec![file("a.txt", "hello")]);
	assert_eq!(loaded(&files).digest, tree_digest(&files));
}

#[rstest]
#[case::no_manifest(Rejection::NoManifest, "AGENT_PLUGIN_NO_MANIFEST")]
#[case::invalid_manifest(
	Rejection::InvalidManifest("expected value".into()),
	"AGENT_PLUGIN_INVALID_MANIFEST: expected value"
)]
#[case::version(
	Rejection::UnsupportedVersion("2.0.0".into()),
	"AGENT_PLUGIN_UNSUPPORTED_VERSION: 2.0.0"
)]
#[case::schema(
	Rejection::SchemaViolation("name".into()),
	"AGENT_PLUGIN_SCHEMA_VIOLATION: name"
)]
#[case::path(Rejection::InvalidPath("a//b".into()), "AGENT_PLUGIN_INVALID_PATH: a//b")]
#[case::limit(Rejection::PackageLimit, "AGENT_PLUGIN_PACKAGE_LIMIT")]
fn rejections_become_invalid_errors_with_a_code(
	#[case] rejection: Rejection,
	#[case] expected: &str,
) {
	assert_eq!(Error::from(rejection), Error::Invalid(expected.into()));
}
