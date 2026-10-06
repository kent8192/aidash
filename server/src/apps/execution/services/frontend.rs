//! Serve the React bundle through Reinhardt's confined local storage.
use crate::config::settings::ProjectSettings;
use http::header::IF_NONE_MATCH;
use reinhardt::utils::storage::{LocalStorage, Storage, StorageError};
use reinhardt::{Request, Response, StatusCode, injectable};
use sha2::{Digest, Sha256};
use std::{io::ErrorKind, path::Path as FilePath, sync::Arc};

#[derive(Clone)]
pub struct Frontend {
	storage: Arc<LocalStorage>,
}

#[injectable(scope = "singleton")]
pub async fn provide_frontend(#[inject] settings: ProjectSettings) -> Frontend {
	Frontend {
		storage: Arc::new(LocalStorage::new(
			settings.core.base_dir.join(&settings.node.web_dir),
			"/",
		)),
	}
}

impl Frontend {
	pub async fn response(&self, request: &Request, path: &str, head: bool) -> Response {
		// API namespaces and hidden files never fall back to the application shell.
		let mut parts = path.split('/');
		let first = parts.next().unwrap_or_default();
		if matches!(first, "api" | "auth" | "federation")
			|| path.contains('\\')
			|| path.split('/').any(|part| part.starts_with('.'))
			|| path.starts_with('/')
		{
			return Response::new(StatusCode::NOT_FOUND);
		}
		let mut file_name = if path.is_empty() { "index.html" } else { path };
		let mut file = self.storage.read(file_name).await;
		if matches!(&file, Err(StorageError::NotFound(_)))
			&& FilePath::new(path).extension().is_none()
			&& first != "assets"
		{
			file_name = "index.html";
			file = self.storage.read(file_name).await;
		}
		let file = match file {
			Ok(file) => file,
			Err(
				StorageError::NotFound(_)
				| StorageError::InvalidPath(_)
				| StorageError::PermissionDenied(_),
			) => {
				return Response::new(StatusCode::NOT_FOUND);
			}
			Err(StorageError::Io(error))
				if matches!(
					error.kind(),
					ErrorKind::NotFound | ErrorKind::PermissionDenied | ErrorKind::IsADirectory
				) =>
			{
				return Response::new(StatusCode::NOT_FOUND);
			}
			Err(error) => {
				tracing::error!(%error, "frontend asset read failed");
				return Response::new(StatusCode::INTERNAL_SERVER_ERROR);
			}
		};
		let etag = format!("\"{:x}\"", Sha256::digest(&file.content));
		let unchanged = request
			.headers
			.get(IF_NONE_MATCH)
			.and_then(|value| value.to_str().ok())
			.is_some_and(|value| {
				value.split(',').any(|tag| {
					let tag = tag.trim();
					tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag
				})
			});
		let status = if unchanged {
			StatusCode::NOT_MODIFIED
		} else {
			StatusCode::OK
		};
		let content_type = mime_guess::from_path(file_name).first_or_octet_stream();
		let response = Response::new(status)
			.with_header("Content-Type", content_type.as_ref())
			.with_header("Cache-Control", "no-cache")
			.with_header("ETag", &etag)
			.with_header("X-Content-Type-Options", "nosniff");
		if unchanged {
			response
		} else if head {
			response.with_header("Content-Length", &file.content.len().to_string())
		} else {
			response.with_body(file.content)
		}
	}
}
