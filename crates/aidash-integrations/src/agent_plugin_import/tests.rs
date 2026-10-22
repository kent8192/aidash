use super::*;
use aidash_domain::agent_plugin::{self, PLUGIN_SCHEMA};
use repository::{COMMIT, Repository, Served};
use rstest::rstest;
use serde_json::json;

mod repository;

const REPOSITORY: &str = "https://github.com/acme/plugins";
const MIB: usize = 1024 * 1024;

fn request(url: &str, plugin_path: Option<&str>) -> PluginImportRequest {
	PluginImportRequest {
		url: url.to_owned(),
		plugin_path: plugin_path.map(str::to_owned),
	}
}

async fn fetch(
	repository: Repository,
	url: &str,
	plugin_path: Option<&str>,
) -> (Result<PluginFetch>, Served) {
	let served = Served::start(repository).await;
	let result = fetch_with_endpoints(request(url, plugin_path), &served.endpoints).await;
	(result, served)
}

fn fetched(result: Result<PluginFetch>) -> FetchedPlugin {
	match result {
		Ok(PluginFetch::Fetched(plugin)) => plugin,
		other => panic!("expected a fetched plugin, got {other:?}"),
	}
}

fn refused(result: Result<PluginFetch>) -> String {
	match result {
		Err(Error::Invalid(message)) => message,
		other => panic!("expected an invalid request, got {other:?}"),
	}
}

fn file(path: &str, content: &str) -> PluginFile {
	PluginFile {
		path: path.to_owned(),
		bytes: content.as_bytes().to_vec(),
	}
}

fn manifest(name: &str) -> String {
	json!({"$schema": PLUGIN_SCHEMA, "name": name}).to_string()
}

fn skill(name: &str) -> String {
	format!("---\nname: {name}\ndescription: Does a thing.\n---\n\nInstructions.\n")
}

/// Adds a plugin directory with a manifest and one Skill.
fn with_plugin(repository: Repository, root: &str, name: &str) -> Repository {
	let at = |path: &str| {
		if root.is_empty() {
			path.to_owned()
		} else {
			format!("{root}/{path}")
		}
	};
	repository
		.file(&at("plugin.json"), manifest(name))
		.file(&at(&format!("skills/{name}/SKILL.md")), skill(name))
}

fn monorepo() -> Repository {
	let repository = with_plugin(Repository::default(), "", "root");
	let repository = with_plugin(repository, "plugins/review", "review");
	with_plugin(repository, "plugins/triage", "triage")
}

#[tokio::test]
async fn a_repository_with_one_plugin_at_its_root_is_fetched_at_the_resolved_commit() {
	let repository = with_plugin(Repository::default(), "", "review")
		.executable("scripts/run.sh", "echo hi")
		.file("README.md", "docs");
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	let plugin = fetched(result);
	assert_eq!(plugin.root, "");
	assert_eq!(
		plugin.source,
		format!("https://github.com/acme/plugins/tree/{COMMIT}")
	);
	assert_eq!(
		plugin.files,
		[
			file("README.md", "docs"),
			file("plugin.json", &manifest("review")),
			file("scripts/run.sh", "echo hi"),
			file("skills/review/SKILL.md", &skill("review")),
		]
	);
	assert!(plugin.denied.is_empty());
	let mut downloads = served.downloads();
	downloads.sort();
	assert_eq!(
		downloads,
		[
			"README.md",
			"plugin.json",
			"scripts/run.sh",
			"skills/review/SKILL.md"
		]
	);
	let loaded = agent_plugin::load(&plugin.files).expect("the fetched files load");
	assert_eq!(loaded.manifest.name.as_str(), "review");
	assert_eq!(loaded.skills.len(), 1);
}

#[tokio::test]
async fn a_repository_with_several_plugins_asks_for_a_choice_before_downloading() {
	let (result, served) = fetch(monorepo(), REPOSITORY, None).await;
	assert_eq!(
		result.unwrap(),
		PluginFetch::Choose(vec![
			String::new(),
			"plugins/review".into(),
			"plugins/triage".into()
		])
	);
	assert!(served.downloads().is_empty());
}

#[tokio::test]
async fn plugins_are_listed_root_first_then_by_path_in_whatever_order_the_tree_arrives() {
	let (result, _) = fetch(monorepo().reversed(), REPOSITORY, None).await;
	assert_eq!(
		result.unwrap(),
		PluginFetch::Choose(vec![
			String::new(),
			"plugins/review".into(),
			"plugins/triage".into()
		])
	);
}

#[rstest]
#[case::root_as_dot(".", "")]
#[case::root_as_empty("", "")]
#[case::directory("plugins/review", "plugins/review")]
#[case::directory_with_trailing_slash("plugins/review/", "plugins/review")]
#[case::another_directory("plugins/triage", "plugins/triage")]
#[tokio::test]
async fn a_chosen_plugin_is_fetched_with_paths_relative_to_its_root(
	#[case] requested: &str,
	#[case] root: &str,
) {
	let (result, _) = fetch(monorepo(), REPOSITORY, Some(requested)).await;
	let plugin = fetched(result);
	assert_eq!(plugin.root, root);
	let expected_source = if root.is_empty() {
		format!("https://github.com/acme/plugins/tree/{COMMIT}")
	} else {
		format!("https://github.com/acme/plugins/tree/{COMMIT}/{root}")
	};
	assert_eq!(plugin.source, expected_source);
	let paths: Vec<&str> = plugin.files.iter().map(|file| file.path.as_str()).collect();
	if root.is_empty() {
		assert_eq!(
			paths,
			[
				"plugin.json",
				"plugins/review/plugin.json",
				"plugins/review/skills/review/SKILL.md",
				"plugins/triage/plugin.json",
				"plugins/triage/skills/triage/SKILL.md",
				"skills/root/SKILL.md",
			]
		);
	} else {
		let name = root.trim_start_matches("plugins/");
		assert_eq!(
			paths,
			["plugin.json".to_owned(), format!("skills/{name}/SKILL.md")]
		);
		assert_eq!(plugin.files[0], file("plugin.json", &manifest(name)));
	}
}

#[rstest]
#[case::not_a_plugin_directory("plugins", "AGENT_PLUGIN_NOT_FOUND: plugins")]
#[case::missing_directory("plugins/nope", "AGENT_PLUGIN_NOT_FOUND: plugins/nope")]
#[tokio::test]
async fn a_requested_directory_that_is_not_a_plugin_is_not_found(
	#[case] requested: &str,
	#[case] message: &str,
) {
	let (result, served) = fetch(monorepo(), REPOSITORY, Some(requested)).await;
	assert_eq!(refused(result), message);
	assert!(served.downloads().is_empty());
}

#[tokio::test]
async fn a_repository_without_a_plugin_has_nothing_to_import() {
	let repository = Repository::default().file("README.md", "docs");
	let (result, _) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(refused(result), "AGENT_PLUGIN_NOT_FOUND");
}

#[tokio::test]
async fn a_tree_url_offers_only_the_plugins_below_it() {
	let (result, _) = fetch(
		monorepo(),
		"https://github.com/acme/plugins/tree/main/plugins",
		None,
	)
	.await;
	assert_eq!(
		result.unwrap(),
		PluginFetch::Choose(vec!["plugins/review".into(), "plugins/triage".into()])
	);
}

#[tokio::test]
async fn a_tree_url_to_a_plugin_directory_fetches_it_without_a_choice() {
	let (result, _) = fetch(
		monorepo(),
		"https://github.com/acme/plugins/tree/v1/plugins/review",
		None,
	)
	.await;
	let plugin = fetched(result);
	assert_eq!(plugin.root, "plugins/review");
	assert_eq!(plugin.files.len(), 2);
}

#[rstest]
#[case::repository_root(".")]
#[case::sibling("plugins/triage")]
#[tokio::test]
async fn a_tree_url_does_not_reach_plugins_outside_its_directory(#[case] requested: &str) {
	let (result, served) = fetch(
		monorepo(),
		"https://github.com/acme/plugins/tree/main/plugins/review",
		Some(requested),
	)
	.await;
	assert_eq!(
		refused(result),
		format!("AGENT_PLUGIN_NOT_FOUND: {requested}")
	);
	assert!(served.downloads().is_empty());
}

#[tokio::test]
async fn a_tree_url_to_a_missing_directory_is_not_found() {
	let (result, _) = fetch(
		monorepo(),
		"https://github.com/acme/plugins/tree/main/plugins/nope",
		None,
	)
	.await;
	assert_eq!(refused(result), "AGENT_PLUGIN_NOT_FOUND: plugins/nope");
}

#[tokio::test]
async fn a_blob_url_to_a_manifest_selects_its_directory_even_with_plugins_nested_in_it() {
	let repository = with_plugin(monorepo(), "plugins/review/extras", "extras");
	let (result, _) = fetch(
		repository,
		"https://github.com/acme/plugins/blob/main/plugins/review/plugin.json",
		None,
	)
	.await;
	let plugin = fetched(result);
	assert_eq!(plugin.root, "plugins/review");
	let paths: Vec<&str> = plugin.files.iter().map(|file| file.path.as_str()).collect();
	assert_eq!(
		paths,
		[
			"extras/plugin.json",
			"extras/skills/extras/SKILL.md",
			"plugin.json",
			"skills/review/SKILL.md",
		]
	);
}

#[tokio::test]
async fn a_blob_url_to_the_root_manifest_selects_the_repository_root() {
	let (result, _) = fetch(
		monorepo(),
		"https://github.com/acme/plugins/blob/main/plugin.json",
		None,
	)
	.await;
	assert_eq!(fetched(result).root, "");
}

#[tokio::test]
async fn a_blob_url_cannot_be_combined_with_another_plugin_directory() {
	let (result, _) = fetch(
		monorepo(),
		"https://github.com/acme/plugins/blob/main/plugins/review/plugin.json",
		Some("plugins/triage"),
	)
	.await;
	assert_eq!(refused(result), "AGENT_PLUGIN_NOT_FOUND: plugins/triage");
}

#[rstest]
#[case::another_file("blob/main/README.md")]
#[case::a_file_that_only_ends_like_the_manifest("blob/main/myplugin.json")]
#[tokio::test]
async fn a_blob_url_must_name_the_manifest(#[case] path: &str) {
	let repository = monorepo()
		.file("README.md", "docs")
		.file("myplugin.json", "{}");
	let (result, _) = fetch(
		repository,
		&format!("https://github.com/acme/plugins/{path}"),
		None,
	)
	.await;
	assert_eq!(
		refused(result),
		"AGENT_PLUGIN_SOURCE: a blob URL must point to plugin.json"
	);
}

#[tokio::test]
async fn dependency_and_repository_metadata_directories_are_not_plugins() {
	let repository = with_plugin(Repository::default(), "plugins/a", "a")
		.file("node_modules/dep/plugin.json", manifest("dep"))
		.file("plugins/a/node_modules/x/plugin.json", manifest("x"))
		.file("vendor/.git/plugin.json", manifest("git"))
		.file("plugins/a/.git/config", "[core]");
	let (result, _) = fetch(repository, REPOSITORY, None).await;
	let plugin = fetched(result);
	assert_eq!(plugin.root, "plugins/a");
	let paths: Vec<&str> = plugin.files.iter().map(|file| file.path.as_str()).collect();
	assert_eq!(
		paths,
		[
			"node_modules/x/plugin.json",
			"plugin.json",
			"skills/a/SKILL.md"
		]
	);
}

#[tokio::test]
async fn at_most_sixty_four_plugins_are_listed_but_every_one_can_be_chosen() {
	let mut repository = Repository::default();
	for index in 0..70 {
		repository = repository.file(&format!("p{index:02}/plugin.json"), manifest("many"));
	}
	let (result, _) = fetch(repository, REPOSITORY, None).await;
	let PluginFetch::Choose(listed) = result.unwrap() else {
		panic!("several plugins ask for a choice");
	};
	assert_eq!(listed.len(), 64);
	assert_eq!(listed[0], "p00");
	assert_eq!(listed[63], "p63");

	let mut repository = Repository::default();
	for index in 0..70 {
		repository = repository.file(&format!("p{index:02}/plugin.json"), manifest("many"));
	}
	let (result, _) = fetch(repository, REPOSITORY, Some("p69")).await;
	assert_eq!(fetched(result).root, "p69");
}

#[tokio::test]
async fn a_symlink_to_a_regular_file_inside_the_plugin_receives_the_files_bytes() {
	let repository = Repository::default()
		.file("plugin.json", manifest("links"))
		.file("docs/readme.md", "readme")
		.link("alias.md", "docs/readme.md")
		.link("nested/up.md", "../docs/readme.md")
		.link("nested/dot.md", "./../docs/./readme.md");
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	let plugin = fetched(result);
	assert_eq!(
		plugin.files,
		[
			file("alias.md", "readme"),
			file("docs/readme.md", "readme"),
			file("nested/dot.md", "readme"),
			file("nested/up.md", "readme"),
			file("plugin.json", &manifest("links")),
		]
	);
	assert!(plugin.denied.is_empty());
	assert!(agent_plugin::load(&plugin.files).is_ok());
	assert_eq!(
		served
			.downloads()
			.iter()
			.filter(|path| path.as_str() == "docs/readme.md")
			.count(),
		1,
		"the target is downloaded once however many links name it"
	);
}

#[tokio::test]
async fn a_symlink_that_leaves_the_plugin_or_names_no_regular_file_is_denied() {
	let repository = Repository::default()
		.file("plugin.json", manifest("links"))
		.file("docs/readme.md", "readme")
		.file("etc/passwd", "inside")
		.link("alias.md", "docs/readme.md")
		.link("escape", "../outside.txt")
		.link("absolute", "/etc/passwd")
		.link("chain", "alias.md")
		.link("to-directory", "docs")
		.link("missing", "nope.md")
		.link("here", ".");
	let (result, _) = fetch(repository, REPOSITORY, None).await;
	let plugin = fetched(result);
	assert_eq!(
		plugin.denied,
		[
			"absolute",
			"chain",
			"escape",
			"here",
			"missing",
			"to-directory"
		]
	);
	let paths: Vec<&str> = plugin.files.iter().map(|file| file.path.as_str()).collect();
	assert_eq!(
		paths,
		["alias.md", "docs/readme.md", "etc/passwd", "plugin.json"]
	);
}

#[tokio::test]
async fn a_symlink_into_a_sibling_plugin_is_denied_even_though_the_file_is_in_the_repository() {
	let repository = monorepo().link("plugins/review/leak.json", "../triage/plugin.json");
	let (result, served) = fetch(repository, REPOSITORY, Some("plugins/review")).await;
	let plugin = fetched(result);
	assert_eq!(plugin.denied, ["leak.json"]);
	let paths: Vec<&str> = plugin.files.iter().map(|file| file.path.as_str()).collect();
	assert_eq!(paths, ["plugin.json", "skills/review/SKILL.md"]);
	assert!(
		!served
			.downloads()
			.contains(&"plugins/triage/plugin.json".to_owned())
	);
}

#[tokio::test]
async fn a_submodule_is_denied_and_never_downloaded() {
	let repository = with_plugin(Repository::default(), "", "review").submodule("vendor/lib");
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	let plugin = fetched(result);
	assert_eq!(plugin.denied, ["vendor/lib"]);
	assert!(
		plugin
			.files
			.iter()
			.all(|file| !file.path.starts_with("vendor"))
	);
	assert!(
		served
			.downloads()
			.iter()
			.all(|path| !path.starts_with("vendor"))
	);
}

fn files_of_size(count: usize) -> Repository {
	let mut repository = Repository::default().file("plugin.json", manifest("big"));
	for index in 1..count {
		repository = repository.file(&format!("files/{index:03}.txt"), "x");
	}
	repository
}

#[tokio::test]
async fn a_plugin_of_exactly_the_file_limit_is_fetched() {
	let (result, _) = fetch(files_of_size(512), REPOSITORY, None).await;
	assert_eq!(fetched(result).files.len(), 512);
}

#[tokio::test]
async fn more_files_than_the_limit_are_refused_before_any_download() {
	let (result, served) = fetch(files_of_size(513), REPOSITORY, None).await;
	assert_eq!(refused(result), "AGENT_PLUGIN_PACKAGE_LIMIT");
	assert!(served.downloads().is_empty());
}

#[tokio::test]
async fn more_symlinks_than_the_file_limit_are_refused_before_any_download() {
	let mut repository = Repository::default().file("plugin.json", manifest("links"));
	for index in 0..512 {
		repository = repository.link(&format!("links/{index:03}"), "../plugin.json");
	}
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(refused(result), "AGENT_PLUGIN_PACKAGE_LIMIT");
	assert!(served.downloads().is_empty());
}

#[tokio::test]
async fn announced_sizes_over_the_byte_limit_are_refused_before_any_download() {
	let repository = Repository::default()
		.file("plugin.json", manifest("big"))
		.announcing("a.bin", 5 * MIB as u64, Vec::new())
		.announcing("b.bin", 4 * MIB as u64, Vec::new());
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(refused(result), "AGENT_PLUGIN_PACKAGE_LIMIT");
	assert!(served.downloads().is_empty());
}

#[tokio::test]
async fn a_download_that_passes_the_remaining_budget_stops_the_import() {
	let repository = Repository::default()
		.file("plugin.json", manifest("big"))
		.file("a.bin", vec![b'a'; 5 * MIB])
		.announcing("b.bin", 1, vec![b'b'; 4 * MIB])
		.file("c.bin", "never requested");
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(refused(result), "AGENT_PLUGIN_PACKAGE_LIMIT");
	assert!(!served.downloads().contains(&"c.bin".to_owned()));
}

#[tokio::test]
async fn a_symlink_copy_counts_against_the_byte_limit() {
	let repository = Repository::default()
		.file("plugin.json", manifest("big"))
		.file("big.bin", vec![b'a'; 5 * MIB])
		.link("copy.bin", "big.bin");
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(refused(result), "AGENT_PLUGIN_PACKAGE_LIMIT");
	assert!(!served.downloads().contains(&"big.bin".to_owned()));
}

#[tokio::test]
async fn a_hidden_repository_is_not_found() {
	let repository = with_plugin(Repository::default(), "", "review").hidden();
	let (result, _) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(refused(result), "public GitHub repository was not found");
}

#[tokio::test]
async fn a_private_repository_is_refused() {
	let repository = with_plugin(Repository::default(), "", "review").private();
	let (result, served) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(
		refused(result),
		"only public GitHub repositories are supported"
	);
	assert!(served.downloads().is_empty());
}

#[tokio::test]
async fn a_truncated_tree_is_refused() {
	let repository = with_plugin(Repository::default(), "", "review").truncated();
	let (result, _) = fetch(repository, REPOSITORY, None).await;
	assert_eq!(
		refused(result),
		"GitHub tree is truncated; use a smaller source"
	);
}

#[rstest]
#[case::skills_sh("https://skills.sh/acme/plugins")]
#[case::skills_sh_skill("https://www.skills.sh/acme/plugins/review")]
#[case::raw_file("https://raw.githubusercontent.com/acme/plugins/main/plugin.json")]
#[case::other_forge("https://gitlab.com/acme/plugins")]
#[tokio::test]
async fn only_github_repositories_carry_agent_plugins(#[case] url: &str) {
	let (result, served) = fetch(monorepo(), url, None).await;
	assert_eq!(
		refused(result),
		"AGENT_PLUGIN_SOURCE: only github.com repositories carry Agent Plugins"
	);
	assert!(served.requests().is_empty());
}

#[rstest]
#[case::cleartext("http://github.com/acme/plugins")]
#[case::query("https://github.com/acme/plugins?ref=main")]
#[case::not_a_url("acme/plugins")]
#[case::no_repository("https://github.com/acme")]
#[tokio::test]
async fn a_github_url_must_be_a_public_https_repository_url(#[case] url: &str) {
	let (result, served) = fetch(monorepo(), url, None).await;
	assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
	assert!(served.requests().is_empty());
}
