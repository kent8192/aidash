use super::*;
use crate::registry::SkillFile;
use rstest::rstest;

fn plugin_with(skills: Vec<PluginFile>) -> LoadedPlugin {
	let mut files = vec![manifest_file("demo")];
	files.extend(skills);
	loaded(&files)
}

fn only_skill_finding(plugin: &LoadedPlugin) -> (Component, Outcome, Reason) {
	assert!(plugin.skills.is_empty(), "the Skill should not load");
	assert_eq!(plugin.findings.len(), 1, "{:?}", plugin.findings);
	let finding = &plugin.findings[0];
	(finding.component.clone(), finding.outcome, finding.reason)
}

fn assert_skipped(files: Vec<PluginFile>, reason: Reason) {
	let plugin = plugin_with(files);
	assert_eq!(
		only_skill_finding(&plugin),
		(Component::Skill("alpha".into()), Outcome::Skipped, reason)
	);
}

fn skill_with(frontmatter: &str) -> PluginFile {
	file(
		"skills/alpha/SKILL.md",
		&format!("---\n{frontmatter}\n---\nBody\n"),
	)
}

#[test]
fn a_skill_is_its_instructions_metadata_and_other_files() {
	let instructions = "---\nname: alpha\ndescription: Does a thing.\nlicense: MIT\ncompatibility: Needs git\nmetadata:\n  author: ada\n  version: \"1.0\"\nallowed-tools: Bash(git:*) Read\nx-unknown: ignored\n---\n\n# Alpha\n";
	let plugin = plugin_with(vec![
		file("skills/alpha/SKILL.md", instructions),
		file("skills/alpha/references/guide.md", "guide"),
		file("skills/alpha/scripts/run.sh", "echo hi"),
	]);
	assert_eq!(plugin.findings, []);
	assert_eq!(
		plugin.skills,
		[PluginSkill {
			directory: "alpha".into(),
			name: "alpha".into(),
			description: "Does a thing.".into(),
			license: Some("MIT".into()),
			compatibility: Some("Needs git".into()),
			instructions: instructions.into(),
			files: vec![
				SkillFile {
					path: "references/guide.md".into(),
					content: "guide".into(),
					encoding: None
				},
				SkillFile {
					path: "scripts/run.sh".into(),
					content: "echo hi".into(),
					encoding: None
				},
			],
		}]
	);
}

#[test]
fn skills_are_listed_by_directory() {
	let plugin = plugin_with(vec![skill("zeta"), skill("alpha"), skill("mid-1")]);
	let names: Vec<_> = plugin
		.skills
		.iter()
		.map(|skill| skill.directory.as_str())
		.collect();
	assert_eq!(names, ["alpha", "mid-1", "zeta"]);
}

#[test]
fn skills_are_not_searched_for_below_one_directory_level() {
	let plugin = plugin_with(vec![
		file(
			"skills/group/nested/SKILL.md",
			&skill_md("nested", "Too deep."),
		),
		file("skills/SKILL.md", &skill_md("skills", "No directory.")),
		file("skills/alpha/skill.md", &skill_md("alpha", "Wrong case.")),
		file(
			"docs/skills/beta/SKILL.md",
			&skill_md("beta", "Not at the root."),
		),
		file("SKILL.md", &skill_md("root", "The plugin root.")),
	]);
	assert_eq!((plugin.skills, plugin.findings), (vec![], vec![]));
}

#[test]
fn a_deeper_skill_file_belongs_to_the_skill_above_it() {
	let plugin = plugin_with(vec![
		skill("alpha"),
		file(
			"skills/alpha/nested/SKILL.md",
			&skill_md("nested", "Not loaded."),
		),
	]);
	assert_eq!(plugin.findings, []);
	assert_eq!(plugin.skills.len(), 1);
	let paths: Vec<_> = plugin.skills[0]
		.files
		.iter()
		.map(|f| f.path.as_str())
		.collect();
	assert_eq!(paths, ["nested/SKILL.md"]);
}

#[test]
fn files_of_other_skills_are_not_mixed_in() {
	let plugin = plugin_with(vec![
		skill("alpha"),
		file("skills/alpha/a.txt", "a"),
		skill("alpha2"),
		file("skills/alpha2/b.txt", "b"),
		file("skills/alphabet.txt", "c"),
	]);
	let files: Vec<_> = plugin
		.skills
		.iter()
		.map(|skill| (skill.directory.as_str(), skill.files.len()))
		.collect();
	assert_eq!(files, [("alpha", 1), ("alpha2", 1)]);
}

#[rstest]
#[case::mismatch("name: other\ndescription: d")]
#[case::missing_name("description: d")]
#[case::missing_description("name: alpha")]
#[case::empty_description("name: alpha\ndescription: \"\"")]
#[case::name_not_a_string("name: [alpha]\ndescription: d")]
#[case::description_not_a_string("name: alpha\ndescription: [d]")]
#[case::license_not_a_string("name: alpha\ndescription: d\nlicense: [MIT]")]
#[case::metadata_not_a_map("name: alpha\ndescription: d\nmetadata: text")]
#[case::metadata_value_not_a_string("name: alpha\ndescription: d\nmetadata:\n  k: [1]")]
#[case::allowed_tools_not_a_string("name: alpha\ndescription: d\nallowed-tools: [Read]")]
#[case::empty_compatibility("name: alpha\ndescription: d\ncompatibility: \"\"")]
#[case::invalid_yaml("name: alpha\ndescription: [unclosed")]
#[case::not_a_mapping("- name\n- description")]
fn a_skill_that_breaks_the_agent_skills_rules_is_skipped(#[case] frontmatter: &str) {
	assert_skipped(vec![skill_with(frontmatter)], Reason::SkillInvalid);
}

#[rstest]
#[case::uppercase("Alpha")]
#[case::underscore("al_pha")]
#[case::leading_hyphen("-alpha")]
#[case::trailing_hyphen("alpha-")]
#[case::double_hyphen("al--pha")]
#[case::dot("al.pha")]
#[case::too_long(&"a".repeat(65))]
fn a_skill_name_follows_the_agent_skills_pattern(#[case] name: &str) {
	let plugin = plugin_with(vec![file(
		&format!("skills/{name}/SKILL.md"),
		&skill_md(name, "d"),
	)]);
	assert_eq!(
		only_skill_finding(&plugin),
		(
			Component::Skill(name.into()),
			Outcome::Skipped,
			Reason::SkillInvalid
		)
	);
}

#[test]
fn the_longest_valid_name_loads() {
	let name = "a".repeat(64);
	let plugin = plugin_with(vec![skill(&name)]);
	assert_eq!(plugin.skills[0].name, name);
}

#[rstest]
#[case::missing_opening("name: alpha\ndescription: d\n---\nBody")]
#[case::missing_closing("---\nname: alpha\ndescription: d\nBody")]
#[case::empty_file("")]
#[case::only_body("# Alpha\n")]
fn a_skill_needs_frontmatter(#[case] text: &str) {
	assert_skipped(
		vec![file("skills/alpha/SKILL.md", text)],
		Reason::SkillInvalid,
	);
}

#[test]
fn a_skill_that_is_not_utf8_is_skipped() {
	let files = vec![PluginFile {
		path: "skills/alpha/SKILL.md".into(),
		bytes: b"---\nname: alpha\ndescription: d\n---\n\xff".to_vec(),
	}];
	assert_skipped(files, Reason::SkillInvalid);
}

#[test]
fn description_and_compatibility_are_counted_in_characters() {
	let kana = "あ";
	let frontmatter = format!(
		"name: alpha\ndescription: {}\ncompatibility: {}",
		kana.repeat(1024),
		kana.repeat(500)
	);
	let plugin = plugin_with(vec![skill_with(&frontmatter)]);
	assert_eq!(plugin.skills[0].description.chars().count(), 1024);

	let too_long = format!("name: alpha\ndescription: {}", "a".repeat(1025));
	assert_skipped(vec![skill_with(&too_long)], Reason::SkillInvalid);
	let too_long = format!(
		"name: alpha\ndescription: d\ncompatibility: {}",
		"a".repeat(501)
	);
	assert_skipped(vec![skill_with(&too_long)], Reason::SkillInvalid);
}

#[test]
fn crlf_frontmatter_loads_and_the_text_is_kept_as_written() {
	let text = "---\r\nname: alpha\r\ndescription: Works.\r\n---\r\n\r\nBody\r\n";
	let plugin = plugin_with(vec![file("skills/alpha/SKILL.md", text)]);
	assert_eq!(plugin.skills[0].description, "Works.");
	assert_eq!(plugin.skills[0].instructions, text);
}

#[test]
fn text_stays_text_and_anything_else_is_base64() {
	let mut files = vec![
		skill("alpha"),
		file("skills/alpha/notes.md", "ノート"),
		PluginFile {
			path: "skills/alpha/logo.png".into(),
			bytes: vec![0x89, b'P', b'N', b'G', 0xff, 0x00],
		},
		PluginFile {
			path: "skills/alpha/nul.txt".into(),
			bytes: b"a\0b".to_vec(),
		},
		file("skills/alpha/empty.txt", ""),
	];
	files.reverse();
	let plugin = plugin_with(files);
	let encoded = |path: &str, content: &str, encoding: Option<&str>| SkillFile {
		path: path.into(),
		content: content.into(),
		encoding: encoding.map(String::from),
	};
	assert_eq!(
		plugin.skills[0].files,
		[
			encoded("empty.txt", "", None),
			encoded("logo.png", "iVBOR/8A", Some("base64")),
			encoded("notes.md", "ノート", None),
			encoded("nul.txt", "YQBi", Some("base64")),
		]
	);
}

#[test]
fn a_skill_md_of_the_size_limit_loads_and_one_byte_more_does_not() {
	let head = "---\nname: alpha\ndescription: d\n---\n";
	let at_limit = format!("{head}{}", "x".repeat(65_536 - head.len()));
	assert_eq!(
		plugin_with(vec![file("skills/alpha/SKILL.md", &at_limit)])
			.skills
			.len(),
		1
	);
	let over = format!("{at_limit}x");
	assert_skipped(
		vec![file("skills/alpha/SKILL.md", &over)],
		Reason::SkillLimit,
	);
}

#[test]
fn nul_in_skill_md_is_over_the_limit() {
	let text = "---\nname: alpha\ndescription: d\n---\na\0b";
	assert_skipped(
		vec![file("skills/alpha/SKILL.md", text)],
		Reason::SkillLimit,
	);
}

#[test]
fn frontmatter_is_limited_to_8192_bytes() {
	let padding = "k".repeat(8192);
	let frontmatter = format!("name: alpha\ndescription: d\n# {padding}");
	assert_skipped(vec![skill_with(&frontmatter)], Reason::SkillLimit);
	let fits = format!("name: alpha\ndescription: d\n# {}", "k".repeat(8000));
	assert_eq!(plugin_with(vec![skill_with(&fits)]).skills.len(), 1);
}

#[test]
fn sixty_three_other_files_load_and_sixty_four_do_not() {
	let others = |count: usize| {
		let mut files = vec![skill("alpha")];
		files.extend((0..count).map(|n| file(&format!("skills/alpha/f{n}.txt"), "x")));
		files
	};
	assert_eq!(plugin_with(others(63)).skills[0].files.len(), 63);
	assert_skipped(others(64), Reason::SkillLimit);
}

#[test]
fn a_skill_of_256000_bytes_loads_and_one_byte_more_does_not() {
	let md = skill("alpha");
	let room = 256_000 - md.bytes.len();
	let sized = |extra: usize| {
		vec![
			md.clone(),
			PluginFile {
				path: "skills/alpha/a.bin".into(),
				bytes: vec![b'a'; room / 2],
			},
			PluginFile {
				path: "skills/alpha/b.bin".into(),
				bytes: vec![b'b'; room - room / 2 + extra],
			},
		]
	};
	assert_eq!(plugin_with(sized(0)).skills.len(), 1);
	assert_skipped(sized(1), Reason::SkillLimit);
}

#[rstest]
#[case::hidden_file("skills/alpha/.env")]
#[case::hidden_directory("skills/alpha/.git/config")]
fn a_dot_prefixed_skill_file_skips_the_skill(#[case] path: &str) {
	assert_skipped(vec![skill("alpha"), file(path, "x")], Reason::SkillLimit);
}

#[test]
fn a_bad_skill_does_not_stop_the_others() {
	let plugin = plugin_with(vec![
		skill("alpha"),
		file("skills/broken/SKILL.md", "no frontmatter"),
		skill("gamma"),
	]);
	assert_eq!(plugin.skills.len(), 2);
	assert_eq!(
		plugin
			.findings
			.iter()
			.map(|f| (f.component.clone(), f.reason))
			.collect::<Vec<_>>(),
		[(Component::Skill("broken".into()), Reason::SkillInvalid)]
	);
}

fn numbered(count: usize) -> Vec<PluginFile> {
	(0..count).map(|n| skill(&format!("s{n:02}"))).collect()
}

#[test]
fn sixteen_skills_load() {
	let plugin = plugin_with(numbered(16));
	assert_eq!((plugin.skills.len(), plugin.findings), (16, vec![]));
}

#[test]
fn skills_past_the_sixteenth_are_skipped_in_directory_order() {
	let mut files = numbered(18);
	files.reverse();
	let plugin = plugin_with(files);
	assert_eq!(plugin.skills.len(), 16);
	assert_eq!(plugin.skills.last().unwrap().directory, "s15");
	assert_eq!(
		plugin
			.findings
			.iter()
			.map(|f| (f.component.clone(), f.outcome, f.reason))
			.collect::<Vec<_>>(),
		["s16", "s17"].map(|dir| (
			Component::Skill(dir.into()),
			Outcome::Skipped,
			Reason::SkillLimit
		))
	);
}

#[test]
fn an_invalid_skill_still_counts_toward_the_sixteen() {
	let mut files = numbered(17);
	files[0] = file("skills/s00/SKILL.md", "no frontmatter");
	let plugin = plugin_with(files);
	assert_eq!(plugin.skills.len(), 15);
	let reasons: Vec<_> = plugin
		.findings
		.iter()
		.map(|f| (f.component.clone(), f.reason))
		.collect();
	assert_eq!(
		reasons,
		[
			(Component::Skill("s00".into()), Reason::SkillInvalid),
			(Component::Skill("s16".into()), Reason::SkillLimit),
		]
	);
}
