//! Resolve public Skill packages from GitHub or skills.sh without executing
//! repository content or forwarding user-controlled URLs to arbitrary hosts.
use crate::{Error, Result};
use aidash_application::ports::registry::SkillSource;
use aidash_domain::registry::skill_import::{
	ImportRequest, ImportResult, ImportedSkill, validate_imported_instructions,
};
use aidash_domain::registry::{SkillFile, rules::valid_skill_file_path};
use async_trait::async_trait;
use base64::Engine;
use reqwest::{Client, Url};
use std::time::Duration;
use tokio::io::AsyncReadExt;

const MAX_TREE_BYTES: usize = 3_000_000;
const MAX_FILE_BYTES: usize = 256_000;
const MAX_FILES: usize = 64;

struct Source {
	owner: String,
	repo: String,
	ref_and_path: Vec<String>,
	explicit: bool,
	blob: bool,
}

#[derive(Clone)]
struct GitHubEndpoints {
	web: Url,
	api: Url,
	raw: Url,
}

impl GitHubEndpoints {
	fn official() -> Self {
		Self {
			web: Url::parse("https://github.com/").expect("static GitHub web URL"),
			api: Url::parse("https://api.github.com/").expect("static GitHub API URL"),
			raw: Url::parse("https://raw.githubusercontent.com/").expect("static GitHub raw URL"),
		}
	}
}

impl Source {
	fn parse(value: &str) -> Result<Self> {
		let url =
			Url::parse(value).map_err(|_| Error::Invalid("enter a GitHub HTTPS URL".into()))?;
		if url.scheme() != "https"
			|| url.host_str() != Some("github.com")
			|| url.port().is_some()
			|| !url.username().is_empty()
			|| url.password().is_some()
			|| url.query().is_some()
			|| url.fragment().is_some()
			|| value.contains('%')
		{
			return Err(Error::Invalid(
				"only public github.com HTTPS URLs are supported".into(),
			));
		}
		let segments = url
			.path_segments()
			.ok_or_else(|| Error::Invalid("invalid GitHub URL".into()))?
			.filter(|part| !part.is_empty())
			.collect::<Vec<_>>();
		if segments.len() < 2
			|| segments[..2].iter().any(|part| {
				!part
					.bytes()
					.all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
			}) {
			return Err(Error::Invalid(
				"GitHub URL must identify an owner and repository".into(),
			));
		}
		let repo = segments[1].trim_end_matches(".git");
		if repo.is_empty() {
			return Err(Error::Invalid("invalid GitHub repository".into()));
		}
		let explicit = segments.len() > 2;
		if explicit
			&& (!matches!(segments[2], "tree" | "blob")
				|| (segments[2] == "tree" && segments.len() < 4)
				|| (segments[2] == "blob" && segments.len() < 5))
		{
			return Err(Error::Invalid(
				"use a repository, tree, or SKILL.md blob URL".into(),
			));
		}
		Ok(Self {
			owner: segments[0].into(),
			repo: repo.into(),
			ref_and_path: if explicit {
				segments[3..].iter().map(|part| (*part).into()).collect()
			} else {
				vec![]
			},
			explicit,
			blob: explicit && segments[2] == "blob",
		})
	}

	fn api_with_base(&self, base: &Url, suffix: &str) -> Result<Url> {
		let (path, query) = suffix.split_once('?').unwrap_or((suffix, ""));
		let mut url = base.clone();
		{
			let mut segments = url
				.path_segments_mut()
				.map_err(|_| Error::Invalid("invalid GitHub API base URL".into()))?;
			segments
				.pop_if_empty()
				.push("repos")
				.push(&self.owner)
				.push(&self.repo);
			for segment in path.split('/').filter(|segment| !segment.is_empty()) {
				segments.push(segment);
			}
		}
		url.set_query((!query.is_empty()).then_some(query));
		Ok(url)
	}

	#[cfg(test)]
	fn api(&self, suffix: &str) -> String {
		self.api_with_base(
			&Url::parse("https://api.github.com/").expect("static GitHub API URL"),
			suffix,
		)
		.expect("valid GitHub API endpoint")
		.as_str()
		.to_owned()
	}

	fn reference_paths(&self) -> impl Iterator<Item = (String, String)> + '_ {
		let longest = self.ref_and_path.len() - usize::from(self.blob);
		(1..=longest).rev().map(|length| {
			(
				self.ref_and_path[..length].join("/"),
				self.ref_and_path[length..].join("/"),
			)
		})
	}
}

fn client() -> Result<Client> {
	Client::builder()
		.timeout(Duration::from_secs(20))
		.redirect(reqwest::redirect::Policy::none())
		.user_agent("aidash-skill-import")
		.build()
		.map_err(|error| Error::External(error.to_string()))
}

async fn bounded_get(client: &Client, url: &str, limit: usize) -> Result<Vec<u8>> {
	let mut response = client
		.get(url)
		.send()
		.await
		.map_err(|error| Error::External(format!("Skill source request failed: {error}")))?;
	if !response.status().is_success() {
		return Err(Error::Invalid(format!(
			"Skill source returned {} for this URL",
			response.status()
		)));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|error| Error::External(error.to_string()))?
	{
		if bytes.len() + chunk.len() > limit {
			return Err(Error::Invalid("Skill exceeds the import size limit".into()));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}

// The CLI may authenticate, but cannot inherit private operator keys.
fn gh_environment_allowed(name: &std::ffi::OsStr) -> bool {
	let Some(name) = name.to_str() else {
		return false;
	};
	name.starts_with("GH_")
		|| matches!(
			name,
			"PATH"
				| "HOME" | "USER"
				| "LANG" | "LC_ALL"
				| "TMPDIR" | "XDG_CONFIG_HOME"
				| "XDG_CACHE_HOME"
				| "XDG_DATA_HOME"
				| "GITHUB_TOKEN"
				| "SSL_CERT_FILE"
				| "SSL_CERT_DIR"
				| "HTTP_PROXY"
				| "HTTPS_PROXY"
				| "ALL_PROXY"
				| "NO_PROXY" | "http_proxy"
				| "https_proxy"
				| "all_proxy"
				| "no_proxy"
		)
}

async fn gh_api(url: &str, limit: usize, accept: &str) -> Result<Option<Vec<u8>>> {
	let parsed = Url::parse(url).map_err(|error| Error::Invalid(error.to_string()))?;
	let endpoint = format!(
		"{}{}",
		parsed.path().trim_start_matches('/'),
		parsed
			.query()
			.map(|query| format!("?{query}"))
			.unwrap_or_default()
	);
	let mut child = tokio::process::Command::new("gh")
		.env_clear()
		.envs(std::env::vars_os().filter(|(name, _)| gh_environment_allowed(name)))
		.arg("api")
		.arg(endpoint)
		.arg("-H")
		.arg(format!("Accept: {accept}"))
		.env("GH_HOST", "github.com")
		.stdin(std::process::Stdio::null())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::null())
		.kill_on_drop(true)
		.spawn()
		.map_err(|_| Error::Invalid("GitHub API rate limit reached; configure GitHub CLI authentication or GITHUB_TOKEN".into()))?;
	let mut output = Vec::new();
	let stdout = child
		.stdout
		.take()
		.ok_or_else(|| Error::External("GitHub CLI output unavailable".into()))?;
	// Failed ref probes include a small JSON error body even when the successful
	// SHA response is limited to 128 bytes.
	let read_limit = limit.max(512);
	let read = tokio::time::timeout(
		Duration::from_secs(20),
		stdout
			.take((read_limit + 1) as u64)
			.read_to_end(&mut output),
	)
	.await;
	if read.is_err() || output.len() > read_limit {
		let _ = child.kill().await;
		return Err(Error::Invalid(
			"GitHub API response exceeds the import size limit".into(),
		));
	}
	let status = child
		.wait()
		.await
		.map_err(|error| Error::External(error.to_string()))?;
	if !status.success() {
		let body: serde_json::Value = serde_json::from_slice(&output).unwrap_or_default();
		if matches!(body["status"].as_str(), Some("404" | "422"))
			|| matches!(body["status"].as_u64(), Some(404 | 422))
		{
			return Ok(None);
		}
		return Err(Error::Invalid(
			"GitHub API request failed; check the URL and GitHub CLI authentication".into(),
		));
	}
	if output.len() > limit {
		return Err(Error::Invalid(
			"GitHub API response exceeds the import size limit".into(),
		));
	}
	Ok(Some(output))
}

async fn api_get(client: &Client, url: &str, limit: usize) -> Result<Option<Vec<u8>>> {
	api_get_with_accept(client, url, limit, "application/vnd.github+json").await
}

async fn api_get_with_accept(
	client: &Client,
	url: &str,
	limit: usize,
	accept: &str,
) -> Result<Option<Vec<u8>>> {
	let mut response = client.get(url).header("accept", accept);
	if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
		response = response.bearer_auth(token);
	}
	let mut response = response
		.send()
		.await
		.map_err(|error| Error::External(format!("GitHub request failed: {error}")))?;
	if matches!(
		response.status(),
		reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::UNPROCESSABLE_ENTITY
	) {
		return Ok(None);
	}
	if matches!(
		response.status(),
		reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::UNAUTHORIZED
	) {
		return gh_api(url, limit, accept).await;
	}
	if !response.status().is_success() {
		return Err(Error::Invalid(format!(
			"GitHub returned {} for this Skill URL",
			response.status()
		)));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|error| Error::External(error.to_string()))?
	{
		if bytes.len() + chunk.len() > limit {
			return Err(Error::Invalid(
				"GitHub API response exceeds the import size limit".into(),
			));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(Some(bytes))
}

async fn commit(
	client: &Client,
	source: &Source,
	endpoints: &GitHubEndpoints,
	reference: &str,
) -> Result<Option<Commit>> {
	let mut url = source.api_with_base(&endpoints.api, "commits")?;
	url.path_segments_mut()
		.map_err(|_| Error::Invalid("invalid GitHub reference".into()))?
		.push(reference);
	// The normal commit response includes every changed file and can dwarf the
	// selected Skill. GitHub's SHA media type resolves branches and tags without
	// that expanded payload; the Git database endpoint then returns the tree.
	let Some(bytes) =
		api_get_with_accept(client, url.as_str(), 128, "application/vnd.github.sha").await?
	else {
		return Ok(None);
	};
	let sha = std::str::from_utf8(&bytes).map_err(|error| Error::Invalid(error.to_string()))?;
	let sha = sha.trim();
	if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(Error::Invalid("invalid GitHub commit SHA response".into()));
	}
	api_get(
		client,
		source
			.api_with_base(&endpoints.api, &format!("git/commits/{sha}"))?
			.as_str(),
		64_000,
	)
	.await?
	.map(|bytes| serde_json::from_slice(&bytes).map_err(Error::from))
	.transpose()
}

fn candidates(tree: &Tree, selected_dir: Option<&str>) -> Vec<String> {
	let mut paths = tree
		.tree
		.iter()
		.filter(|entry| {
			entry.kind == "blob" && (entry.path == "SKILL.md" || entry.path.ends_with("/SKILL.md"))
		})
		.map(|entry| entry.path.clone())
		.collect::<Vec<_>>();
	if let Some(dir) = selected_dir {
		if let Some(exact) = paths
			.iter()
			.find(|path| *path == dir || *path == &format!("{dir}/SKILL.md"))
		{
			return vec![exact.clone()];
		}
		paths.retain(|path| path.starts_with(&format!("{dir}/")));
	}
	paths.sort_by_key(|path| {
		let parts = path.split('/').collect::<Vec<_>>();
		let preferred = parts.len() == 1
			|| (parts.len() <= 5 && ["skills", ".agents", ".claude", ".codex"].contains(&parts[0]));
		(!preferred, path.clone())
	});
	paths
}

async fn fetch_tree(
	client: &Client,
	source: &Source,
	endpoints: &GitHubEndpoints,
	sha: &str,
	recursive: bool,
) -> Result<Tree> {
	let suffix = if recursive { "?recursive=1" } else { "" };
	let tree: Tree = serde_json::from_slice(
		&api_get(
			client,
			source
				.api_with_base(&endpoints.api, &format!("git/trees/{sha}{suffix}"))?
				.as_str(),
			MAX_TREE_BYTES,
		)
		.await?
		.ok_or_else(|| Error::Invalid("GitHub tree was not found".into()))?,
	)?;
	if tree.truncated {
		return Err(Error::Invalid(
			"GitHub tree is truncated; use a smaller source".into(),
		));
	}
	Ok(tree)
}

async fn skill_tree(
	client: &Client,
	source: &Source,
	endpoints: &GitHubEndpoints,
	root_sha: &str,
	subpath: &str,
) -> Result<Tree> {
	let directory = if source.blob {
		subpath.strip_suffix("/SKILL.md").unwrap_or("")
	} else {
		subpath
	};
	if source.blob && directory.is_empty() {
		let root = fetch_tree(client, source, endpoints, root_sha, false).await?;
		let mut entries = Vec::new();
		for entry in root.tree {
			if entry.kind == "blob" && entry.path == "SKILL.md" {
				entries.push(entry);
			} else if entry.kind == "tree"
				&& ["references", "scripts", "assets", "templates"].contains(&entry.path.as_str())
			{
				let mut subtree = fetch_tree(client, source, endpoints, &entry.sha, true).await?;
				for file in &mut subtree.tree {
					file.path = format!("{}/{}", entry.path, file.path);
				}
				entries.extend(subtree.tree);
			}
		}
		return Ok(Tree {
			tree: entries,
			truncated: false,
		});
	}
	let mut sha = root_sha.to_owned();
	if !directory.is_empty() {
		for segment in directory.split('/') {
			let parent = fetch_tree(client, source, endpoints, &sha, false).await?;
			sha = parent
				.tree
				.into_iter()
				.find(|entry| entry.path == segment && entry.kind == "tree")
				.ok_or_else(|| Error::Invalid("GitHub Skill directory was not found".into()))?
				.sha;
		}
	}
	let mut tree = fetch_tree(client, source, endpoints, &sha, true).await?;
	if !directory.is_empty() {
		for entry in &mut tree.tree {
			entry.path = format!("{directory}/{}", entry.path);
		}
	}
	Ok(tree)
}

fn resource_paths(tree: &Tree, skill_path: &str) -> Result<Vec<String>> {
	let dir = skill_path.strip_suffix("/SKILL.md").unwrap_or("");
	let prefix = if dir.is_empty() {
		String::new()
	} else {
		format!("{dir}/")
	};
	let mut paths = tree
		.tree
		.iter()
		.filter(|entry| entry.kind == "blob")
		.filter_map(|entry| entry.path.strip_prefix(&prefix))
		.filter(|relative| {
			if *relative == "SKILL.md" {
				return true;
			}
			if dir.is_empty() {
				return ["references/", "scripts/", "assets/", "templates/"]
					.iter()
					.any(|prefix| relative.starts_with(prefix));
			}
			true
		})
		.filter(|relative| {
			!relative.split('/').any(|part| {
				part.starts_with('.') || matches!(part, "__pycache__" | "__pypackages__")
			}) && *relative != "metadata.json"
		})
		.map(str::to_owned)
		.collect::<Vec<_>>();
	if paths.iter().any(|path| !valid_skill_file_path(path)) {
		return Err(Error::Invalid(
			"GitHub Skill contains a file path the registry cannot store".into(),
		));
	}
	paths.sort();
	if paths.len() > MAX_FILES {
		return Err(Error::Invalid("Skill contains more than 64 files".into()));
	}
	Ok(paths)
}

fn github_instructions(bytes: Vec<u8>, path: &str) -> Result<String> {
	let instructions = String::from_utf8(bytes)
		.map_err(|_| Error::Invalid(format!("{path} is not a UTF-8 text file")))?;
	validate_imported_instructions(&instructions)?;
	Ok(instructions)
}

fn imported_snapshot(url: &str, snapshot: SkillsShSnapshot) -> Result<ImportResult> {
	if snapshot.files.len() > MAX_FILES {
		return Err(Error::Invalid("Skill contains more than 64 files".into()));
	}
	let mut instructions = None;
	let mut files = Vec::new();
	let mut total = 0;
	let mut paths = std::collections::HashSet::new();
	for file in snapshot.files {
		if !valid_skill_file_path(&file.path)
			|| file.contents.contains('\0')
			|| !paths.insert(file.path.clone())
		{
			return Err(Error::Invalid(
				"registry Skill has an invalid file path".into(),
			));
		}
		total += file.contents.len();
		if total > MAX_FILE_BYTES {
			return Err(Error::Invalid(
				"registry Skill exceeds the import size limit".into(),
			));
		}
		if file.path == "SKILL.md" {
			if instructions.replace(file.contents).is_some() {
				return Err(Error::Invalid(
					"registry Skill has duplicate SKILL.md files".into(),
				));
			}
		} else {
			files.push(SkillFile {
				path: file.path,
				content: file.contents,
				encoding: None,
			});
		}
	}
	let instructions =
		instructions.ok_or_else(|| Error::Invalid("registry Skill has no SKILL.md".into()))?;
	validate_imported_instructions(&instructions)?;
	Ok(ImportResult {
		skills: vec!["SKILL.md".into()],
		selected: Some(ImportedSkill {
			path: "SKILL.md".into(),
			source: url.into(),
			instructions,
			files,
		}),
	})
}

async fn import_skills_sh(
	request: ImportRequest,
	endpoints: &GitHubEndpoints,
) -> Result<ImportResult> {
	let url =
		Url::parse(&request.url).map_err(|_| Error::Invalid("invalid skills.sh URL".into()))?;
	if url.scheme() != "https"
		|| !matches!(url.host_str(), Some("skills.sh" | "www.skills.sh"))
		|| url.port().is_some()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.query().is_some()
		|| url.fragment().is_some()
		|| request.url.contains('%')
	{
		return Err(Error::Invalid(
			"use a public https://skills.sh/owner/repo/skill URL".into(),
		));
	}
	let parts = url
		.path_segments()
		.ok_or_else(|| Error::Invalid("invalid skills.sh URL".into()))?
		.filter(|part| !part.is_empty())
		.collect::<Vec<_>>();
	if !matches!(parts.len(), 2 | 3)
		|| parts.iter().any(|part| {
			!part
				.bytes()
				.all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
		}) {
		return Err(Error::Invalid(
			"use a skills.sh Skill page URL with owner, repository, and Skill name".into(),
		));
	}
	if parts.len() == 2 {
		if parts[0] == "p" {
			return Err(Error::Invalid(
				"skills.sh pack URLs are not supported yet".into(),
			));
		}
		return import_github(
			ImportRequest {
				url: format!("https://github.com/{}/{}", parts[0], parts[1]),
				skill_path: request.skill_path,
			},
			endpoints,
		)
		.await;
	}
	if request
		.skill_path
		.as_deref()
		.is_some_and(|path| path != "SKILL.md")
	{
		return Err(Error::Invalid(
			"selected Skill does not match this registry page".into(),
		));
	}
	let download_url = format!(
		"https://www.skills.sh/api/download/{}/{}/{}",
		parts[0], parts[1], parts[2]
	);
	let data = bounded_get(&client()?, &download_url, 400_000).await?;
	let snapshot: SkillsShSnapshot = serde_json::from_slice(&data)
		.map_err(|_| Error::Invalid("skills.sh returned an invalid Skill snapshot".into()))?;
	imported_snapshot(&request.url, snapshot)
}

/// Public GitHub and skills.sh transport with bounded, non-executing imports.
#[derive(Default)]
pub struct PublicSkillSource;

#[async_trait]
impl SkillSource for PublicSkillSource {
	async fn import(&self, request: ImportRequest) -> Result<ImportResult> {
		import_with_endpoints(request, &GitHubEndpoints::official()).await
	}
}

async fn import_with_endpoints(
	request: ImportRequest,
	endpoints: &GitHubEndpoints,
) -> Result<ImportResult> {
	let host = Url::parse(&request.url)
		.ok()
		.and_then(|url| url.host_str().map(str::to_owned));
	match host.as_deref() {
		Some("github.com") => import_github(request, endpoints).await,
		Some("raw.githubusercontent.com") => {
			import_github(
				ImportRequest {
					url: raw_github_to_blob(&request.url)?,
					skill_path: request.skill_path,
				},
				endpoints,
			)
			.await
		}
		Some("skills.sh" | "www.skills.sh") => import_skills_sh(request, endpoints).await,
		_ => Err(Error::Invalid("use a GitHub or skills.sh Skill URL".into())),
	}
}

fn raw_github_to_blob(value: &str) -> Result<String> {
	let url = Url::parse(value).map_err(|_| Error::Invalid("invalid GitHub raw URL".into()))?;
	if url.scheme() != "https"
		|| url.host_str() != Some("raw.githubusercontent.com")
		|| url.port().is_some()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.query().is_some()
		|| url.fragment().is_some()
		|| value.contains('%')
	{
		return Err(Error::Invalid("invalid GitHub raw URL".into()));
	}
	let parts = url
		.path_segments()
		.ok_or_else(|| Error::Invalid("invalid GitHub raw URL".into()))?
		.collect::<Vec<_>>();
	if parts.len() < 4 || parts.last() != Some(&"SKILL.md") {
		return Err(Error::Invalid(
			"GitHub raw URL must point to SKILL.md".into(),
		));
	}
	let blob = format!(
		"https://github.com/{}/{}/blob/{}",
		parts[0],
		parts[1],
		parts[2..].join("/")
	);
	Source::parse(&blob)?;
	Ok(blob)
}

async fn import_github(
	request: ImportRequest,
	endpoints: &GitHubEndpoints,
) -> Result<ImportResult> {
	let source = Source::parse(&request.url)?;
	let client = client()?;
	// The GitHub web page is checked without credentials because the anonymous
	// REST API may already be rate limited. Private repositories return 404.
	let mut public_url = endpoints.web.clone();
	public_url
		.path_segments_mut()
		.map_err(|_| Error::Invalid("invalid GitHub web base URL".into()))?
		.pop_if_empty()
		.push(&source.owner)
		.push(&source.repo);
	let response = client
		.get(public_url)
		.send()
		.await
		.map_err(|error| Error::External(format!("GitHub visibility check failed: {error}")))?;
	if response.status() != reqwest::StatusCode::OK {
		return Err(Error::Invalid(
			"public GitHub repository was not found".into(),
		));
	}
	let repo: Repository = serde_json::from_slice(
		&api_get(
			&client,
			source.api_with_base(&endpoints.api, "")?.as_str(),
			64_000,
		)
		.await?
		.ok_or_else(|| Error::Invalid("public GitHub repository was not found".into()))?,
	)?;
	if repo.private {
		return Err(Error::Invalid(
			"only public GitHub repositories are supported".into(),
		));
	}
	let (commit, subpath) = if source.explicit {
		let mut found = None;
		for (reference, subpath) in source.reference_paths() {
			if let Some(value) = commit(&client, &source, endpoints, &reference).await? {
				found = Some((value, subpath));
				break;
			}
		}
		found.ok_or_else(|| Error::Invalid("GitHub reference was not found".into()))?
	} else {
		let commit = commit(&client, &source, endpoints, &repo.default_branch)
			.await?
			.ok_or_else(|| Error::Invalid("GitHub default branch was not found".into()))?;
		(commit, String::new())
	};
	let explicit_path = if subpath.is_empty() {
		None
	} else {
		Some(subpath.as_str())
	};
	if source.blob && !subpath.ends_with("SKILL.md") {
		return Err(Error::Invalid(
			"GitHub blob URL must point to SKILL.md".into(),
		));
	}
	let tree = skill_tree(&client, &source, endpoints, &commit.tree.sha, &subpath).await?;
	let skills = candidates(&tree, explicit_path);
	if skills.is_empty() {
		return Err(Error::Invalid(
			"no SKILL.md found at this GitHub URL".into(),
		));
	}
	let choice = request.skill_path.clone().or(if skills.len() == 1 {
		Some(skills[0].clone())
	} else {
		None
	});
	let Some(choice) = choice else {
		return Ok(ImportResult {
			skills,
			selected: None,
		});
	};
	if !skills.iter().any(|path| path == &choice) {
		return Err(Error::Invalid(
			"selected SKILL.md is not in this source".into(),
		));
	}
	let dir = choice.strip_suffix("/SKILL.md").unwrap_or("");
	let mut instructions = None;
	let mut files = Vec::new();
	let mut total = 0;
	for path in resource_paths(&tree, &choice)? {
		let full_path = if dir.is_empty() {
			path.clone()
		} else {
			format!("{dir}/{path}")
		};
		let mut raw_url = endpoints.raw.clone();
		{
			let mut segments = raw_url
				.path_segments_mut()
				.map_err(|_| Error::Invalid("invalid Skill path".into()))?;
			segments
				.push(&source.owner)
				.push(&source.repo)
				.push(&commit.sha);
			for segment in full_path.split('/') {
				segments.push(segment);
			}
		}
		let bytes = bounded_get(&client, raw_url.as_str(), MAX_FILE_BYTES - total).await?;
		total += bytes.len();
		if path == "SKILL.md" {
			instructions = Some(github_instructions(bytes, &full_path)?);
		} else {
			let (content, encoding) = match String::from_utf8(bytes) {
				Ok(text) if !text.contains('\0') => (text, None),
				Ok(text) => (
					base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
					Some("base64".into()),
				),
				Err(error) => (
					base64::engine::general_purpose::STANDARD.encode(error.into_bytes()),
					Some("base64".into()),
				),
			};
			files.push(SkillFile {
				path,
				content,
				encoding,
			});
		}
	}
	let instructions =
		instructions.ok_or_else(|| Error::Invalid("SKILL.md could not be read".into()))?;
	let source_url = format!(
		"https://github.com/{}/{}/blob/{}/{}",
		source.owner, source.repo, commit.sha, choice
	);
	Ok(ImportResult {
		skills,
		selected: Some(ImportedSkill {
			path: choice,
			source: source_url,
			instructions,
			files,
		}),
	})
}

#[derive(serde::Deserialize)]
struct Repository {
	default_branch: String,
	private: bool,
}

#[derive(serde::Deserialize)]
struct Commit {
	sha: String,
	tree: TreeSha,
}

#[derive(serde::Deserialize)]
struct TreeSha {
	sha: String,
}

#[derive(serde::Deserialize)]
struct Tree {
	tree: Vec<TreeEntry>,
	truncated: bool,
}

#[derive(serde::Deserialize)]
struct TreeEntry {
	path: String,
	#[serde(rename = "type")]
	kind: String,
	sha: String,
}

#[derive(serde::Deserialize)]
struct SkillsShSnapshot {
	files: Vec<SkillsShFile>,
}

#[derive(serde::Deserialize)]
struct SkillsShFile {
	path: String,
	contents: String,
}

#[cfg(test)]
mod tests;
