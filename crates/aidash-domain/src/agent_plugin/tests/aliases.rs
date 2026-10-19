use super::*;
use rstest::rstest;
use std::collections::BTreeSet;

fn alias(plugin: &str, server: &str, tool: &str) -> String {
	model_alias(&plugin_name(plugin), &server_name(server), tool)
}

#[rstest]
#[case::plain("demo", "srv", "search", "demo__srv__search")]
#[case::hyphen_and_underscore(
	"my-plugin",
	"my_srv",
	"get-item_v2",
	"my-plugin__my_srv__get-item_v2"
)]
#[case::plugin_dots("my.plugin", "srv", "t", "my_plugin__srv__t")]
#[case::server_dots("demo", "a.b", "t", "demo__a_b__t")]
#[case::tool_punctuation("demo", "srv", "get file/v2:x", "demo__srv__get_file_v2_x")]
#[case::one_underscore_per_character("demo", "srv", "ノート", "demo__srv_____")]
#[case::empty_tool("demo", "srv", "", "demo__srv__")]
fn characters_outside_the_allowed_set_become_underscores(
	#[case] plugin: &str,
	#[case] server: &str,
	#[case] tool: &str,
	#[case] expected: &str,
) {
	assert_eq!(alias(plugin, server, tool), expected);
}

#[test]
fn sixty_four_characters_are_kept_whole() {
	let tool = "t".repeat(64 - "demo__srv__".len());
	let alias = alias("demo", "srv", &tool);
	assert_eq!(alias.len(), 64);
	assert_eq!(alias, format!("demo__srv__{tool}"));
}

#[test]
fn a_longer_alias_keeps_55_characters_an_underscore_and_a_hash() {
	let alias = alias("demo", "srv", &"t".repeat(80));
	assert_eq!(alias, format!("demo__srv__{}_b1c34149", "t".repeat(44)));
	assert_eq!(alias.len(), 64);
}

#[test]
fn truncated_aliases_that_share_a_prefix_stay_distinct() {
	let shared = "x".repeat(60);
	let aliases: BTreeSet<_> = ["a", "b", "ab"]
		.iter()
		.map(|suffix| alias("demo", "srv", &format!("{shared}{suffix}")))
		.collect();
	assert_eq!(aliases.len(), 3);
	assert!(aliases.iter().all(|alias| alias.len() == 64));
}

#[test]
fn the_hash_covers_the_raw_inputs_not_the_normalized_alias() {
	let dotted = alias("demo", "srv", &format!("{}.", "t".repeat(80)));
	let underscored = alias("demo", "srv", &format!("{}_", "t".repeat(80)));
	assert_ne!(dotted, underscored);
	assert_eq!(dotted[..55], underscored[..55]);
}

#[test]
fn the_same_inputs_always_give_the_same_alias() {
	let tool = "t".repeat(100);
	assert_eq!(alias("demo", "srv", &tool), alias("demo", "srv", &tool));
}

#[test]
fn inputs_that_normalize_alike_collide_when_nothing_is_truncated() {
	// The caller detects the collision and skips the tool.
	assert_eq!(
		alias("demo", "srv", "get.file"),
		alias("demo", "srv", "get_file")
	);
	assert_eq!(alias("demo", "a.b", "t"), alias("demo", "a_b", "t"));
}
