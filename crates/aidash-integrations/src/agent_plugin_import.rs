//! Fetch one Agent Plugin directory from a public GitHub repository at an exact commit.
//!
//! Repository content is never executed and only the fixed GitHub endpoints are contacted. The
//! files are handed to `aidash_domain::agent_plugin::load`, which decides what is accepted.
use crate::skill_import::{
	Download, GitHubEndpoints, Resolved, Source, TreeEntry, directory_tree, download, raw_file_url,
	resolve,
};
use crate::{Error, Result};
use aidash_application::ports::registry::{
	AgentPluginSource, FetchedPlugin, PluginFetch, PluginImportRequest,
};
use aidash_domain::agent_plugin::PluginFile;
use async_trait::async_trait;
use reqwest::{Client, Url};
use std::collections::{BTreeMap, BTreeSet};

const MANIFEST: &str = "plugin.json";
const MAX_LISTED_ROOTS: usize = 64;
const MAX_FILES: usize = 512;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINK_TARGET_BYTES: usize = 4096;

/// Public GitHub transport for Agent Plugins.
#[derive(Default)]
pub struct PublicAgentPluginSource;

#[async_trait]
impl AgentPluginSource for PublicAgentPluginSource {
	async fn fetch(&self, request: PluginImportRequest) -> Result<PluginFetch> {
		fetch_with_endpoints(request, &GitHubEndpoints::official()).await
	}
}

async fn fetch_with_endpoints(
	request: PluginImportRequest,
	endpoints: &GitHubEndpoints,
) -> Result<PluginFetch> {
	let source = parse_source(&request.url)?;
	let blob = source.blob;
	let Resolved {
		source,
		client,
		commit,
		subpath,
	} = resolve(source, endpoints).await?;
	let scope = Scope::of_url(blob, subpath)?;
	let tree = directory_tree(
		&client,
		&source,
		endpoints,
		&commit.tree.sha,
		scope.directory(),
	)
	.await?
	.ok_or_else(|| Error::Invalid(format!("AGENT_PLUGIN_NOT_FOUND: {}", scope.directory())))?
	.tree;
	let roots = candidates(&tree, &scope);
	let root = match choose(&roots, request.plugin_path.as_deref())? {
		Choice::Several(listed) => return Ok(PluginFetch::Choose(listed)),
		Choice::One(root) => root,
	};
	let retrieved = Members::below(&tree, &root)
		.retrieve(&client, endpoints, &source, &commit.sha)
		.await?;
	let suffix = if root.is_empty() {
		String::new()
	} else {
		format!("/{root}")
	};
	Ok(PluginFetch::Fetched(FetchedPlugin {
		entitlement_expires_at: None,
		source: format!(
			"https://github.com/{}/{}/tree/{}{suffix}",
			source.owner, source.repo, commit.sha
		),
		root,
		files: retrieved.files,
		denied: retrieved.denied,
	}))
}

fn parse_source(url: &str) -> Result<Source> {
	let elsewhere = Url::parse(url)
		.ok()
		.and_then(|parsed| parsed.host_str().map(|host| host != "github.com"))
		.unwrap_or(false);
	if elsewhere {
		return Err(Error::Invalid(
			"AGENT_PLUGIN_SOURCE: only github.com repositories carry Agent Plugins".into(),
		));
	}
	Source::parse_naming(url, MANIFEST)
}

/// Which plugin directories a URL offers.
enum Scope {
	/// Every plugin directory at or below this one; the repository root is the empty path.
	Within(String),
	/// A `blob` URL to a `plugin.json` selects exactly its directory.
	Exactly(String),
}

impl Scope {
	/// `subpath` is the part of the URL after the reference.
	fn of_url(blob: bool, subpath: String) -> Result<Self> {
		if !blob {
			return Ok(Self::Within(subpath));
		}
		match subpath.strip_suffix(MANIFEST) {
			Some("") => Ok(Self::Exactly(String::new())),
			Some(directory) if directory.ends_with('/') => {
				Ok(Self::Exactly(directory.trim_end_matches('/').to_owned()))
			}
			_ => Err(Error::Invalid(
				"AGENT_PLUGIN_SOURCE: a blob URL must point to plugin.json".into(),
			)),
		}
	}

	fn directory(&self) -> &str {
		match self {
			Self::Within(directory) | Self::Exactly(directory) => directory,
		}
	}

	fn admits(&self, directory: &str) -> bool {
		match self {
			Self::Within(base) => {
				base.is_empty()
					|| directory == base
					|| directory
						.strip_prefix(base.as_str())
						.is_some_and(|rest| rest.starts_with('/'))
			}
			Self::Exactly(only) => directory == only,
		}
	}
}

/// Plugin roots, the repository root first and the rest by path.
fn candidates(tree: &[TreeEntry], scope: &Scope) -> Vec<String> {
	let mut roots: Vec<String> = tree
		.iter()
		.filter(|entry| entry.kind == "blob")
		.filter_map(|entry| match entry.path.strip_suffix(MANIFEST) {
			Some("") => Some(String::new()),
			Some(directory) => directory.strip_suffix('/').map(str::to_owned),
			None => None,
		})
		.filter(|root| {
			scope.admits(root)
				&& !root
					.split('/')
					.any(|segment| matches!(segment, "node_modules" | ".git"))
		})
		.collect();
	roots.sort();
	roots
}

enum Choice {
	One(String),
	Several(Vec<String>),
}

fn choose(roots: &[String], requested: Option<&str>) -> Result<Choice> {
	match (requested, roots) {
		(Some(path), _) => {
			let wanted = path.strip_suffix('/').unwrap_or(path);
			let wanted = if wanted == "." { "" } else { wanted };
			roots
				.iter()
				.find(|root| root.as_str() == wanted)
				.map(|root| Choice::One(root.clone()))
				.ok_or_else(|| Error::Invalid(format!("AGENT_PLUGIN_NOT_FOUND: {path}")))
		}
		(None, []) => Err(Error::Invalid("AGENT_PLUGIN_NOT_FOUND".into())),
		(None, [only]) => Ok(Choice::One(only.clone())),
		(None, many) => Ok(Choice::Several(
			many.iter().take(MAX_LISTED_ROOTS).cloned().collect(),
		)),
	}
}

fn package_limit() -> Error {
	Error::Invalid("AGENT_PLUGIN_PACKAGE_LIMIT".into())
}

/// The entries below a plugin root, by what a client may do with each.
struct Members<'a> {
	files: BTreeMap<String, &'a TreeEntry>,
	links: BTreeMap<String, &'a TreeEntry>,
	denied: BTreeSet<String>,
}

/// What `Members::retrieve` returns: sorted files and sorted denied paths.
struct Retrieved {
	files: Vec<PluginFile>,
	denied: Vec<String>,
}

impl<'a> Members<'a> {
	fn below(tree: &'a [TreeEntry], root: &str) -> Self {
		let prefix = if root.is_empty() {
			String::new()
		} else {
			format!("{root}/")
		};
		let mut members = Self {
			files: BTreeMap::new(),
			links: BTreeMap::new(),
			denied: BTreeSet::new(),
		};
		for entry in tree {
			let Some(relative) = entry.path.strip_prefix(&prefix) else {
				continue;
			};
			if relative.split('/').any(|segment| segment == ".git") {
				continue;
			}
			match (entry.kind.as_str(), entry.mode.as_deref()) {
				("tree", _) => {}
				("blob", Some("100644" | "100755")) => {
					members.files.insert(relative.to_owned(), entry);
				}
				("blob", Some("120000")) => {
					members.links.insert(relative.to_owned(), entry);
				}
				// Submodules, and any mode that is not a file or a symlink.
				_ => {
					members.denied.insert(relative.to_owned());
				}
			}
		}
		members
	}

	/// Counts and announced sizes are checked before the first request. A symlink counts as a file
	/// there, so resolving the links cannot raise the count; it only adds the target's size once
	/// more per copy. Regular files are downloaded last against the remaining byte budget.
	async fn retrieve(
		mut self,
		client: &Client,
		endpoints: &GitHubEndpoints,
		source: &Source,
		sha: &str,
	) -> Result<Retrieved> {
		let announced_bytes = announced(self.files.values().copied());
		if self.files.len() + self.links.len() > MAX_FILES || announced_bytes > MAX_BYTES as u64 {
			return Err(package_limit());
		}
		let mut copies: Vec<(String, String)> = Vec::new();
		for (path, entry) in std::mem::take(&mut self.links) {
			match self
				.resolve_link(client, endpoints, source, sha, &path, entry)
				.await?
			{
				Some(target) => copies.push((path, target)),
				None => {
					self.denied.insert(path);
				}
			}
		}
		let copied: u64 = copies
			.iter()
			.filter_map(|(_, target)| self.files[target].size)
			.sum();
		if announced_bytes + copied > MAX_BYTES as u64 {
			return Err(package_limit());
		}
		let mut contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
		let mut used = 0;
		for (path, entry) in &self.files {
			let url = raw_file_url(endpoints, source, sha, &entry.path)?;
			match download(
				client,
				url.as_str(),
				MAX_BYTES - used,
				"Agent Plugin source",
			)
			.await?
			{
				Download::Complete(bytes) => {
					used += bytes.len();
					contents.insert(path.clone(), bytes);
				}
				Download::OverLimit => return Err(package_limit()),
			}
		}
		for (link, target) in copies {
			let bytes = contents[&target].clone();
			used += bytes.len();
			if used > MAX_BYTES {
				return Err(package_limit());
			}
			contents.insert(link, bytes);
		}
		Ok(Retrieved {
			files: contents
				.into_iter()
				.map(|(path, bytes)| PluginFile { path, bytes })
				.collect(),
			denied: self.denied.into_iter().collect(),
		})
	}

	/// The regular file a symlink names, or `None` when the specification forbids access: the
	/// target leaves the plugin root, is not a regular file of the tree, or is another symlink.
	async fn resolve_link(
		&self,
		client: &Client,
		endpoints: &GitHubEndpoints,
		source: &Source,
		sha: &str,
		link: &str,
		entry: &TreeEntry,
	) -> Result<Option<String>> {
		if entry
			.size
			.is_some_and(|size| size > MAX_LINK_TARGET_BYTES as u64)
		{
			return Ok(None);
		}
		let url = raw_file_url(endpoints, source, sha, &entry.path)?;
		let Download::Complete(target) = download(
			client,
			url.as_str(),
			MAX_LINK_TARGET_BYTES,
			"Agent Plugin source",
		)
		.await?
		else {
			return Ok(None);
		};
		Ok(link_target(link, &target).filter(|target| self.files.contains_key(target)))
	}
}

fn announced<'a>(entries: impl Iterator<Item = &'a TreeEntry>) -> u64 {
	entries.filter_map(|entry| entry.size).sum()
}

/// Resolves a symlink's target lexically against the link's directory. `None` when the target
/// is absolute, climbs above the plugin root or names the root itself.
fn link_target(link: &str, target: &[u8]) -> Option<String> {
	let target = std::str::from_utf8(target).ok()?;
	if target.starts_with('/') {
		return None;
	}
	let mut kept: Vec<&str> = link.split('/').collect();
	kept.pop();
	for segment in target.split('/') {
		match segment {
			"" | "." => {}
			".." => {
				kept.pop()?;
			}
			other => kept.push(other),
		}
	}
	(!kept.is_empty()).then(|| kept.join("/"))
}

#[cfg(test)]
mod tests;
