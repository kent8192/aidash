use super::*;
use crate::{
	agent_plugin::fixtures::{SOURCE, plugin, reference},
	capabilities::skills,
};
use rstest::rstest;

#[test]
fn a_reference_records_what_an_approver_must_see() {
	let loaded = plugin("acme-tools", &["lint", "deploy"]);

	let reference = AgentPluginRef::of(&loaded, SOURCE);

	assert_eq!(reference.digest, loaded.digest);
	assert_eq!(reference.name, plugin_name("acme-tools"));
	assert_eq!(reference.source, SOURCE);
	assert_eq!(reference.version.as_deref(), Some("1.0.0"));
	assert_eq!(reference.license.as_deref(), Some("MIT"));
	assert_eq!(reference.skills, ["deploy", "lint"]);
	assert_eq!(reference.servers, loaded.servers);
	assert_eq!(reference.servers.len(), 1);
	assert_eq!(reference.findings, loaded.findings);
	assert_eq!(reference.findings.len(), 1);
}

#[test]
fn a_reference_without_version_or_license_omits_them() {
	let loaded = loaded(&[manifest_file("bare")]);

	let stored = serde_json::to_value(AgentPluginRef::of(&loaded, SOURCE)).unwrap();

	assert!(stored.get("version").is_none());
	assert!(stored.get("license").is_none());
	assert_eq!(
		serde_json::from_value::<AgentPluginRef>(stored).unwrap(),
		AgentPluginRef::of(&loaded, SOURCE)
	);
}

#[test]
fn a_stored_reference_refuses_unknown_fields_and_invalid_names() {
	let mut stored = serde_json::to_value(reference("acme-tools", &["lint"])).unwrap();
	stored["extra"] = json!(true);
	assert!(serde_json::from_value::<AgentPluginRef>(stored).is_err());

	let mut stored = serde_json::to_value(reference("acme-tools", &["lint"])).unwrap();
	stored["name"] = json!("Bad Name");
	assert!(serde_json::from_value::<AgentPluginRef>(stored).is_err());
}

#[rstest]
#[case::accepted(SOURCE, true)]
#[case::not_github("https://gitlab.com/acme/tools", false)]
#[case::not_https("http://github.com/acme/tools", false)]
#[case::too_long(&format!("https://github.com/{}", "a".repeat(MAX_SOURCE_BYTES)), false)]
#[case::longest(&format!("https://github.com/{}", "a".repeat(MAX_SOURCE_BYTES - "https://github.com/".len())), true)]
fn the_source_is_a_bounded_github_url(#[case] source: &str, #[case] accepted: bool) {
	let mut reference = reference("acme-tools", &["lint"]);
	reference.source = source.to_owned();

	assert_eq!(reference.validate().is_ok(), accepted);
}

#[rstest]
#[case::accepted(&format!("sha256:{}", "a".repeat(64)), true)]
#[case::uppercase(&format!("sha256:{}", "A".repeat(64)), false)]
#[case::short(&format!("sha256:{}", "a".repeat(63)), false)]
#[case::long(&format!("sha256:{}", "a".repeat(65)), false)]
#[case::other_algorithm(&format!("sha512:{}", "a".repeat(64)), false)]
#[case::bare(&"a".repeat(64), false)]
fn the_digest_is_sha256_in_lowercase_hex(#[case] digest: &str, #[case] accepted: bool) {
	let mut reference = reference("acme-tools", &["lint"]);
	reference.digest = digest.to_owned();

	assert_eq!(reference.validate().is_ok(), accepted);
}

#[test]
fn a_loaded_digest_is_a_valid_reference_digest() {
	assert!(reference("acme-tools", &["lint"]).validate().is_ok());
}

#[test]
fn a_reference_carries_at_most_sixteen_skill_names() {
	let mut reference = reference("acme-tools", &["lint"]);
	reference.skills = (0..MAX_REFERENCE_SKILLS).map(|n| format!("s{n}")).collect();
	assert!(reference.validate().is_ok());

	reference.skills.push("one-more".into());
	assert!(reference.validate().is_err());
}

#[test]
fn a_plugin_skill_becomes_a_valid_direct_skill_with_a_stable_identity() {
	let loaded = plugin("acme-tools", &["lint"]);
	let skill = &loaded.skills[0];

	let attachment = skill.attachment(&loaded.digest, SOURCE);

	assert_eq!(attachment.origin, format!("{SOURCE}/skills/lint"));
	assert_eq!(attachment.instructions, skill.instructions);
	assert_eq!(attachment.files, skill.files);
	assert_eq!(attachment.digest, skills::content_digest(&attachment));
	assert_eq!(skills::validate(&attachment).unwrap().name, "lint");
	assert_eq!(attachment, skill.attachment(&loaded.digest, SOURCE));
}

#[test]
fn the_skill_id_follows_the_plugin_copy_and_the_directory_only() {
	let loaded = plugin("acme-tools", &["lint", "deploy"]);
	let other = plugin("other-tools", &["lint", "deploy"]);
	let id = |plugin: &LoadedPlugin, index: usize, source: &str| {
		plugin.skills[index]
			.attachment(&plugin.digest, source)
			.skill_id
	};

	assert_ne!(id(&loaded, 0, SOURCE), id(&loaded, 1, SOURCE));
	assert_ne!(id(&loaded, 0, SOURCE), id(&other, 0, SOURCE));
	assert_eq!(
		id(&loaded, 0, SOURCE),
		id(&loaded, 0, "https://github.com/mirror/tools")
	);
	assert_eq!(id(&loaded, 0, SOURCE).get_version_num(), 8);
}

#[test]
fn the_longest_accepted_source_still_yields_a_valid_skill_origin() {
	let loaded = plugin("acme-tools", &["lint"]);
	let skill = PluginSkill {
		directory: "a".repeat(64),
		name: "a".repeat(64),
		..loaded.skills[0].clone()
	};
	let source = format!(
		"https://github.com/{}",
		"a".repeat(MAX_SOURCE_BYTES - "https://github.com/".len())
	);

	let attachment = skill.attachment(&loaded.digest, &source);

	assert!(skills::validate(&attachment).is_ok());
}
