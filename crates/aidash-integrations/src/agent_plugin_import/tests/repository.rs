//! An in-memory GitHub repository served over loopback in the shapes the importer reads.
use crate::skill_import::GitHubEndpoints;
use axum::{
	body::Body,
	extract::State,
	http::{Request, StatusCode},
	response::Response,
	routing::any,
};
use reqwest::Url;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

pub(super) const COMMIT: &str = "0123456789012345678901234567890123456789";

enum Node {
	Blob {
		mode: &'static str,
		content: Vec<u8>,
		declared: u64,
	},
	Submodule,
}

#[derive(Default)]
pub(super) struct Repository {
	nodes: BTreeMap<String, Node>,
	private: bool,
	hidden: bool,
	truncated: bool,
	reversed: bool,
}

impl Repository {
	pub(super) fn file(self, path: &str, content: impl AsRef<[u8]>) -> Self {
		self.blob(path, "100644", content.as_ref().to_vec(), None)
	}

	pub(super) fn executable(self, path: &str, content: impl AsRef<[u8]>) -> Self {
		self.blob(path, "100755", content.as_ref().to_vec(), None)
	}

	pub(super) fn link(self, path: &str, target: &str) -> Self {
		self.blob(path, "120000", target.as_bytes().to_vec(), None)
	}

	/// A file whose tree entry announces `declared` bytes while the download serves `content`.
	pub(super) fn announcing(self, path: &str, declared: u64, content: Vec<u8>) -> Self {
		self.blob(path, "100644", content, Some(declared))
	}

	pub(super) fn submodule(mut self, path: &str) -> Self {
		self.nodes.insert(path.to_owned(), Node::Submodule);
		self
	}

	/// The web page answers 404, as GitHub does for a private repository without credentials.
	pub(super) fn hidden(mut self) -> Self {
		self.hidden = true;
		self
	}

	/// The API reports `private: true`.
	pub(super) fn private(mut self) -> Self {
		self.private = true;
		self
	}

	pub(super) fn truncated(mut self) -> Self {
		self.truncated = true;
		self
	}

	/// Lists tree entries in reverse path order.
	pub(super) fn reversed(mut self) -> Self {
		self.reversed = true;
		self
	}

	fn blob(
		mut self,
		path: &str,
		mode: &'static str,
		content: Vec<u8>,
		declared: Option<u64>,
	) -> Self {
		let declared = declared.unwrap_or(content.len() as u64);
		self.nodes.insert(
			path.to_owned(),
			Node::Blob {
				mode,
				content,
				declared,
			},
		);
		self
	}

	/// The entries of one directory; `None` when the directory does not exist.
	fn tree(&self, directory: &str, recursive: bool) -> Option<Vec<Value>> {
		let prefix = if directory.is_empty() {
			String::new()
		} else {
			format!("{directory}/")
		};
		let mut entries = Vec::new();
		let mut directories = BTreeSet::new();
		let mut exists = directory.is_empty();
		for (path, node) in &self.nodes {
			let Some(relative) = path.strip_prefix(&prefix) else {
				continue;
			};
			exists = true;
			let parts: Vec<&str> = relative.split('/').collect();
			let nested = if recursive {
				parts.len()
			} else {
				2.min(parts.len())
			};
			for depth in 1..nested {
				let below = parts[..depth].join("/");
				if directories.insert(below.clone()) {
					entries.push(json!({
						"path": below,
						"type": "tree",
						"mode": "040000",
						"sha": tree_sha(&join(directory, &below)),
					}));
				}
			}
			if recursive || parts.len() == 1 {
				entries.push(match node {
					Node::Blob { mode, declared, .. } => json!({
						"path": relative,
						"type": "blob",
						"mode": mode,
						"sha": format!("blob-{path}"),
						"size": declared,
					}),
					Node::Submodule => json!({
						"path": relative,
						"type": "commit",
						"mode": "160000",
						"sha": "submodule",
					}),
				});
			}
		}
		if self.reversed {
			entries.reverse();
		}
		exists.then_some(entries)
	}
}

fn join(directory: &str, name: &str) -> String {
	if directory.is_empty() {
		name.to_owned()
	} else {
		format!("{directory}/{name}")
	}
}

fn tree_sha(directory: &str) -> String {
	if directory.is_empty() {
		"tree-root".to_owned()
	} else {
		format!("tree-{}", directory.replace('/', "~"))
	}
}

struct Shared {
	repository: Repository,
	requests: Mutex<Vec<String>>,
}

/// A running fixture. Dropping it stops the server.
pub(super) struct Served {
	pub(super) endpoints: GitHubEndpoints,
	shared: Arc<Shared>,
	server: tokio::task::JoinHandle<()>,
}

impl Drop for Served {
	fn drop(&mut self) {
		self.server.abort();
	}
}

impl Served {
	pub(super) async fn start(repository: Repository) -> Self {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.expect("bind GitHub fixture");
		let origin = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
		let shared = Arc::new(Shared {
			repository,
			requests: Mutex::new(Vec::new()),
		});
		let app = axum::Router::new()
			.fallback(any(respond))
			.with_state(shared.clone());
		let server = tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap();
		});
		Self {
			endpoints: GitHubEndpoints {
				web: origin.clone(),
				api: origin.clone(),
				raw: origin,
			},
			shared,
			server,
		}
	}

	pub(super) fn requests(&self) -> Vec<String> {
		self.shared.requests.lock().unwrap().clone()
	}

	/// Paths requested from the raw file host, without the owner, repository and commit.
	pub(super) fn downloads(&self) -> Vec<String> {
		let prefix = format!("/acme/plugins/{COMMIT}/");
		self.requests()
			.iter()
			.filter_map(|request| request.strip_prefix(&prefix).map(str::to_owned))
			.collect()
	}
}

fn answer(status: StatusCode, content_type: &str, body: impl Into<Body>) -> Response {
	Response::builder()
		.status(status)
		.header("content-type", content_type)
		.body(body.into())
		.unwrap()
}

fn json_answer(value: Value) -> Response {
	answer(StatusCode::OK, "application/json", value.to_string())
}

fn missing() -> Response {
	answer(StatusCode::NOT_FOUND, "text/plain", Body::empty())
}

async fn respond(State(shared): State<Arc<Shared>>, request: Request<Body>) -> Response {
	let uri = request.uri();
	let path = uri.path().to_owned();
	let recursive = uri.query() == Some("recursive=1");
	shared.requests.lock().unwrap().push(path.clone());
	let repository = &shared.repository;
	let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
	match segments.as_slice() {
		["acme", "plugins"] if repository.hidden => missing(),
		["acme", "plugins"] => answer(StatusCode::OK, "text/html", Body::empty()),
		["repos", "acme", "plugins"] => json_answer(json!({
			"default_branch": "main",
			"private": repository.private,
		})),
		["repos", "acme", "plugins", "commits", "main" | "v1"] => {
			answer(StatusCode::OK, "text/plain", COMMIT)
		}
		["repos", "acme", "plugins", "git", "commits", COMMIT] => json_answer(json!({
			"sha": COMMIT,
			"tree": {"sha": "tree-root"},
		})),
		["repos", "acme", "plugins", "git", "trees", tree] => {
			let directory = match tree.strip_prefix("tree-") {
				Some("root") => String::new(),
				Some(directory) => directory.replace('~', "/"),
				None => return missing(),
			};
			match repository.tree(&directory, recursive) {
				Some(entries) => json_answer(json!({
					"truncated": repository.truncated,
					"tree": entries,
				})),
				None => missing(),
			}
		}
		["acme", "plugins", COMMIT, rest @ ..] if !rest.is_empty() => {
			match repository.nodes.get(&rest.join("/")) {
				Some(Node::Blob { content, .. }) => {
					answer(StatusCode::OK, "application/octet-stream", content.clone())
				}
				_ => missing(),
			}
		}
		_ => missing(),
	}
}
