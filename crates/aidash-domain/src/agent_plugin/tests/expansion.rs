use super::*;
use rstest::rstest;

#[rstest]
#[case::root("${PLUGIN_ROOT}/bin/run.js", "/plugins/demo/bin/run.js")]
#[case::data("--cache=${PLUGIN_DATA}/cache", "--cache=/data/demo/cache")]
#[case::both("${PLUGIN_ROOT}:${PLUGIN_DATA}", "/plugins/demo:/data/demo")]
#[case::repeated("${PLUGIN_ROOT}${PLUGIN_ROOT}", "/plugins/demo/plugins/demo")]
#[case::after_a_dollar("$${PLUGIN_DATA}", "$/data/demo")]
#[case::nothing_to_expand("--verbose", "--verbose")]
#[case::empty("", "")]
#[case::other_placeholder("${OTHER}/${HOME}", "${OTHER}/${HOME}")]
#[case::lowercase("${plugin_root}", "${plugin_root}")]
#[case::shell_style("$PLUGIN_ROOT", "$PLUGIN_ROOT")]
#[case::unterminated("${PLUGIN_ROOT", "${PLUGIN_ROOT")]
#[case::trailing_opener("a${", "a${")]
#[case::nested("${${PLUGIN_ROOT}}", "${/plugins/demo}")]
#[case::multibyte("日本${PLUGIN_ROOT}語", "日本/plugins/demo語")]
fn placeholders_are_replaced_once_and_the_rest_stays_literal(
	#[case] value: &str,
	#[case] expected: &str,
) {
	assert_eq!(expand(value, "/plugins/demo", "/data/demo"), expected);
}

#[test]
fn replacement_text_is_never_scanned_again() {
	assert_eq!(
		expand(
			"${PLUGIN_ROOT}|${PLUGIN_DATA}",
			"/r/${PLUGIN_DATA}",
			"/d/${PLUGIN_ROOT}"
		),
		"/r/${PLUGIN_DATA}|/d/${PLUGIN_ROOT}"
	);
	assert_eq!(
		expand("${PLUGIN_ROOT}", "${PLUGIN_ROOT}", "x"),
		"${PLUGIN_ROOT}"
	);
}
