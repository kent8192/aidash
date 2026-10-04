use super::*;
use axum::{
	body::Body,
	http::{Request, StatusCode},
	routing::any,
};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};

const FIXTURE_COMMIT_SHA: &str = "0123456789012345678901234567890123456789";

#[derive(Clone)]
struct GitHubMockState {
	requests: Arc<std::sync::Mutex<Vec<String>>>,
	overflow_reference: Arc<AtomicBool>,
}

struct GitHubImportFixture {
	endpoints: GitHubEndpoints,
	state: GitHubMockState,
	server: tokio::task::JoinHandle<()>,
}

impl Drop for GitHubImportFixture {
	fn drop(&mut self) {
		self.server.abort();
	}
}

#[rstest::fixture]
async fn github_import_fixture() -> GitHubImportFixture {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind GitHub mock fixture");
	let origin = Url::parse(&format!("http://{}", listener.local_addr().unwrap()))
		.expect("local GitHub mock URL");
	let endpoints = GitHubEndpoints {
		web: origin.clone(),
		api: origin.clone(),
		raw: origin,
	};
	let state = GitHubMockState {
		requests: Arc::new(std::sync::Mutex::new(Vec::new())),
		overflow_reference: Arc::new(AtomicBool::new(false)),
	};
	let app = axum::Router::new()
		.fallback(any(github_mock_response))
		.with_state(state.clone());
	let server = tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	});
	GitHubImportFixture {
		endpoints,
		state,
		server,
	}
}

async fn github_mock_response(
	axum::extract::State(state): axum::extract::State<GitHubMockState>,
	request: Request<Body>,
) -> axum::response::Response {
	let uri = request.uri();
	let path = uri.path().to_owned();
	let requested = format!(
		"{path}{}",
		uri.query()
			.map(|query| format!("?{query}"))
			.unwrap_or_default()
	);
	state.requests.lock().unwrap().push(requested);

	let (status, content_type, body) = match path.as_str() {
		"/openai/skills" => (StatusCode::OK, "text/html", Vec::new()),
		"/repos/openai/skills" => (
			StatusCode::OK,
			"application/json",
			br#"{"default_branch":"main","private":false}"#.to_vec(),
		),
		"/repos/openai/skills/commits/main" => (
			StatusCode::OK,
			"text/plain",
			FIXTURE_COMMIT_SHA.as_bytes().to_vec(),
		),
		"/repos/openai/skills/git/commits/0123456789012345678901234567890123456789" => (
			StatusCode::OK,
			"application/json",
			format!(r#"{{"sha":"{FIXTURE_COMMIT_SHA}","tree":{{"sha":"root-tree"}}}}"#)
				.into_bytes(),
		),
		"/repos/openai/skills/git/trees/root-tree" => (
			StatusCode::OK,
			"application/json",
			br#"{"truncated":false,"tree":[{"path":"skills","type":"tree","sha":"skills-tree"}]}"#.to_vec(),
		),
		"/repos/openai/skills/git/trees/skills-tree" => (
			StatusCode::OK,
			"application/json",
			br#"{"truncated":false,"tree":[{"path":".curated","type":"tree","sha":"curated-tree"}]}"#.to_vec(),
		),
		"/repos/openai/skills/git/trees/curated-tree" => (
			StatusCode::OK,
			"application/json",
			br#"{"truncated":false,"tree":[{"path":"aspnet-core","type":"tree","sha":"aspnet-tree"}]}"#.to_vec(),
		),
		"/repos/openai/skills/git/trees/aspnet-tree" => (
			StatusCode::OK,
			"application/json",
			br#"{"truncated":false,"tree":[{"path":"SKILL.md","type":"blob","sha":"skill"},{"path":"references/stack-selection.md","type":"blob","sha":"reference"},{"path":"assets/icon.png","type":"blob","sha":"binary"}]}"#.to_vec(),
		),
		path if path.ends_with("/skills/.curated/aspnet-core/SKILL.md") => (
			StatusCode::OK,
			"text/plain",
			b"---\nname: aspnet-core\ndescription: Mocked import\n---\nUse references/stack-selection.md"
				.to_vec(),
		),
		path if path.ends_with("/skills/.curated/aspnet-core/assets/icon.png") => (
			StatusCode::OK,
			"application/octet-stream",
			vec![0x00, 0xff, 0x01, 0x80],
		),
		path if path.ends_with("/skills/.curated/aspnet-core/references/stack-selection.md") => {
			let body = if state.overflow_reference.load(Ordering::SeqCst) {
				vec![b'x'; 256_000]
			} else {
				b"Use the approved framework version.".to_vec()
			};
			(StatusCode::OK, "text/plain", body)
		}
		_ => (StatusCode::NOT_FOUND, "text/plain", Vec::new()),
	};
	axum::response::Response::builder()
		.status(status)
		.header("content-type", content_type)
		.body(Body::from(body))
		.unwrap()
}

#[rstest::fixture]
fn skills_sh_snapshot() -> (&'static str, SkillsShSnapshot) {
	(
		"https://skills.sh/vercel-labs/skills/find-skills",
		SkillsShSnapshot {
			files: vec![
				SkillsShFile {
					path: "SKILL.md".into(),
					contents:
						"---\nname: find-skills\ndescription: Find reusable skills\n---\nBody"
							.into(),
				},
				SkillsShFile {
					path: "references/catalog.md".into(),
					contents: "Catalog fixture".into(),
				},
			],
		},
	)
}

#[rstest::fixture]
fn github_skill_tree() -> Tree {
	Tree {
		truncated: false,
		tree: vec![
			TreeEntry {
				path: "skills/.curated/aspnet-core/SKILL.md".into(),
				kind: "blob".into(),
				sha: "skill".into(),
			},
			TreeEntry {
				path: "skills/.curated/aspnet-core/references/stack-selection.md".into(),
				kind: "blob".into(),
				sha: "reference".into(),
			},
			TreeEntry {
				path: "skills/.curated/aspnet-core/assets/icon.png".into(),
				kind: "blob".into(),
				sha: "binary".into(),
			},
		],
	}
}

#[rstest::rstest]
#[tokio::test]
async fn github_import_fetches_commits_tree_files_and_enforces_total_size(
	#[future(awt)]
	#[from(github_import_fixture)]
	fixture: GitHubImportFixture,
) {
	let request = || ImportRequest {
		url: "https://github.com/openai/skills/tree/main/skills/.curated/aspnet-core".into(),
		skill_path: None,
	};
	let result = import_with_endpoints(request(), &fixture.endpoints)
		.await
		.expect("import mocked GitHub Skill");
	assert_eq!(result.skills, vec!["skills/.curated/aspnet-core/SKILL.md"]);
	let selected = result.selected.expect("single Skill is selected");
	assert!(selected.instructions.contains("name: aspnet-core"));
	assert_eq!(
		selected.source,
		format!(
			"https://github.com/openai/skills/blob/{FIXTURE_COMMIT_SHA}/skills/.curated/aspnet-core/SKILL.md"
		)
	);
	assert_eq!(
		selected
			.files
			.iter()
			.map(|file| file.path.as_str())
			.collect::<Vec<_>>(),
		vec!["assets/icon.png", "references/stack-selection.md"]
	);
	assert_eq!(selected.files[0].content, "AP8BgA==");
	assert_eq!(selected.files[0].encoding.as_deref(), Some("base64"));
	assert_eq!(
		selected.files[1].content,
		"Use the approved framework version."
	);
	let requests = fixture.state.requests.lock().unwrap().clone();
	assert!(requests.contains(&"/openai/skills".into()));
	assert!(requests.contains(&"/repos/openai/skills".into()));
	assert!(requests.contains(&format!(
		"/repos/openai/skills/git/commits/{FIXTURE_COMMIT_SHA}"
	)));
	assert!(requests.contains(&"/repos/openai/skills/git/trees/aspnet-tree?recursive=1".into()));
	assert!(requests.contains(&format!(
		"/openai/skills/{FIXTURE_COMMIT_SHA}/skills/.curated/aspnet-core/SKILL.md"
	)));

	fixture
		.state
		.overflow_reference
		.store(true, Ordering::SeqCst);
	let error = import_with_endpoints(request(), &fixture.endpoints)
		.await
		.expect_err("aggregate Skill size exceeds the importer limit");
	assert!(
		error
			.to_string()
			.contains("Skill exceeds the import size limit")
	);
}

#[rstest::rstest]
fn accepts_only_restricted_github_sources() {
	assert!(Source::parse("https://github.com/openai/skills/tree/main/skills/foo").is_ok());
	let root_tree = Source::parse("https://github.com/openai/skills/tree/release").unwrap();
	assert_eq!(root_tree.ref_and_path, vec!["release"]);
	assert!(root_tree.explicit);
	assert!(!root_tree.blob);
	assert_eq!(
		root_tree.reference_paths().collect::<Vec<_>>(),
		vec![("release".into(), String::new())]
	);
	let slashed_ref = Source::parse("https://github.com/openai/skills/tree/release/v1").unwrap();
	assert_eq!(
		slashed_ref.reference_paths().collect::<Vec<_>>(),
		vec![
			("release/v1".into(), String::new()),
			("release".into(), "v1".into())
		]
	);
	let skill_directory =
		Source::parse("https://github.com/openai/skills/tree/release/skills/foo").unwrap();
	assert_eq!(
		skill_directory.reference_paths().collect::<Vec<_>>(),
		vec![
			("release/skills/foo".into(), String::new()),
			("release/skills".into(), "foo".into()),
			("release".into(), "skills/foo".into())
		]
	);
	let skill_blob =
		Source::parse("https://github.com/openai/skills/blob/release/SKILL.md").unwrap();
	assert_eq!(
		skill_blob.reference_paths().collect::<Vec<_>>(),
		vec![("release".into(), "SKILL.md".into())]
	);
	let source = Source::parse("https://github.com/openai/skills").unwrap();
	assert_eq!(source.api(""), "https://api.github.com/repos/openai/skills");
	let mut commit_url = Url::parse(&source.api("commits")).unwrap();
	commit_url.path_segments_mut().unwrap().push("feature/docs");
	assert_eq!(
		commit_url.path(),
		"/repos/openai/skills/commits/feature%2Fdocs"
	);
	let mut raw_url = Url::parse("https://raw.githubusercontent.com").unwrap();
	raw_url
		.path_segments_mut()
		.unwrap()
		.push("openai")
		.push("skills");
	assert_eq!(raw_url.path(), "/openai/skills");
	for url in [
		"http://github.com/openai/skills",
		"https://github.com.evil.test/openai/skills",
		"https://github.com/openai/skills?foo=1",
		"https://github.com/openai/skills/tree/main/%2e%2e",
	] {
		assert!(Source::parse(url).is_err(), "{url}");
	}
	assert_eq!(
		raw_github_to_blob(
			"https://raw.githubusercontent.com/openai/skills/main/skills/foo/SKILL.md"
		)
		.unwrap(),
		"https://github.com/openai/skills/blob/main/skills/foo/SKILL.md"
	);
	assert!(
		raw_github_to_blob("https://raw.githubusercontent.com/openai/skills/main/README.md")
			.is_err()
	);
}

#[rstest::rstest]
fn converts_registry_snapshot_without_losing_reference_files() {
	let result = imported_snapshot(
		"https://skills.sh/example/repo/research",
		SkillsShSnapshot {
			files: vec![
				SkillsShFile {
					path: "SKILL.md".into(),
					contents:
						"---\nname: research\ndescription: Test\n---\nRead references/guide.md"
							.into(),
				},
				SkillsShFile {
					path: "references/guide.md".into(),
					contents: "Guide".into(),
				},
			],
		},
	)
	.unwrap();
	let selected = result.selected.unwrap();
	assert!(selected.instructions.contains("references/guide.md"));
	assert_eq!(selected.files[0].path, "references/guide.md");
	assert_eq!(selected.files[0].content, "Guide");
}

#[rstest::rstest]
fn imports_skills_sh_snapshot(
	#[from(skills_sh_snapshot)] (url, snapshot): (&str, SkillsShSnapshot),
) {
	let result = imported_snapshot(url, snapshot).unwrap();
	assert!(
		result
			.selected
			.unwrap()
			.instructions
			.contains("name: find-skills")
	);
}

#[rstest::rstest]
fn imports_github_skill_directory_fixture(github_skill_tree: Tree) {
	let selected = "skills/.curated/aspnet-core/SKILL.md";
	assert_eq!(
		candidates(&github_skill_tree, Some(selected)),
		vec![selected]
	);
	assert_eq!(
		resource_paths(&github_skill_tree, selected).unwrap(),
		vec![
			"SKILL.md",
			"assets/icon.png",
			"references/stack-selection.md"
		]
	);
	assert!(
		github_instructions(
			b"---\nname: aspnet-core\n---\nUse references/stack-selection.md".to_vec(),
			selected,
		)
		.unwrap()
		.contains("name: aspnet-core")
	);
}

#[rstest::rstest]
fn discovers_nested_skills_and_keeps_only_selected_directory() {
	let tree = Tree {
		truncated: false,
		tree: vec![
			TreeEntry {
				path: "skills/one/SKILL.md".into(),
				kind: "blob".into(),
				sha: "one".into(),
			},
			TreeEntry {
				path: "skills/one/references/guide.md".into(),
				kind: "blob".into(),
				sha: "guide".into(),
			},
			TreeEntry {
				path: "skills/two/SKILL.md".into(),
				kind: "blob".into(),
				sha: "two".into(),
			},
			TreeEntry {
				path: "packages/editor/SKILL.md".into(),
				kind: "blob".into(),
				sha: "editor".into(),
			},
		],
	};
	assert_eq!(
		candidates(&tree, None),
		vec![
			"skills/one/SKILL.md",
			"skills/two/SKILL.md",
			"packages/editor/SKILL.md"
		]
	);
	assert_eq!(
		candidates(&tree, Some("skills/one")),
		vec!["skills/one/SKILL.md"]
	);
	assert_eq!(
		candidates(&tree, Some("skills")),
		vec!["skills/one/SKILL.md", "skills/two/SKILL.md"]
	);
	assert_eq!(
		resource_paths(&tree, "skills/one/SKILL.md").unwrap(),
		vec!["SKILL.md", "references/guide.md"]
	);
}

#[rstest::rstest]
fn rejects_github_resource_paths_that_registry_cannot_store() {
	let tree_with = |path: String| Tree {
		truncated: false,
		tree: vec![
			TreeEntry {
				path: "skills/example/SKILL.md".into(),
				kind: "blob".into(),
				sha: "skill".into(),
			},
			TreeEntry {
				path: format!("skills/example/{path}"),
				kind: "blob".into(),
				sha: "resource".into(),
			},
		],
	};
	let prefix = "references/";
	let longest = format!("{prefix}{}", "界".repeat(76)) + "x";
	assert_eq!(longest.len(), 240);
	assert!(resource_paths(&tree_with(longest), "skills/example/SKILL.md").is_ok());
	for path in [
		format!("{prefix}{}", "界".repeat(76)) + "xx",
		"references/bad\\name.md".into(),
		"references/bad\nname.md".into(),
		"references//guide.md".into(),
	] {
		assert!(
			resource_paths(&tree_with(path.clone()), "skills/example/SKILL.md").is_err(),
			"{path:?}"
		);
	}
}

#[rstest::rstest]
fn rejects_github_instructions_that_registry_cannot_store() {
	assert_eq!(
		github_instructions(b"---\nname: example\n---\nBody".to_vec(), "SKILL.md").unwrap(),
		"---\nname: example\n---\nBody"
	);
	for bytes in [
		b"---\nname: example\n---\nBad\0body".to_vec(),
		vec![b'x'; 65_537],
		b"  \n".to_vec(),
		vec![0xff],
	] {
		assert!(github_instructions(bytes, "SKILL.md").is_err());
	}
	assert!(github_instructions(vec![b'x'; 65_536], "SKILL.md").is_ok());
}
