//! Static Pages assets are served by the same owning product process.
use crate::{Error, Marketplace, Result};
use reinhardt::Response;
impl Marketplace {
	pub(crate) fn assets(&self, path: &str) -> Result<Response> {
		let root = self.config.assets_dir.as_ref().ok_or(Error::NotFound)?;
		// Only generated assets and the shell are public. Never expose settings,
		// credentials, source files or symlink targets outside the bundle.
		if path != "index.html"
			&& (!path.starts_with("assets/")
				|| path.split('/').any(|part| {
					part.is_empty() || part == ".." || part == "." || part.contains('\\')
				})) {
			return Err(Error::NotFound);
		}
		let root = root.canonicalize().map_err(|_| Error::NotFound)?;
		let file = root
			.join(path)
			.canonicalize()
			.map_err(|_| Error::NotFound)?;
		if !file.starts_with(&root) || !file.is_file() {
			return Err(Error::NotFound);
		}
		let mime = match file.extension().and_then(|extension| extension.to_str()) {
			Some("html") => "text/html; charset=utf-8",
			Some("js") => "text/javascript; charset=utf-8",
			Some("css") => "text/css; charset=utf-8",
			Some("wasm") => "application/wasm",
			Some("woff2") => "font/woff2",
			Some("woff") => "font/woff",
			Some("svg") => "image/svg+xml",
			_ => "application/octet-stream",
		};
		let body = std::fs::read(file).map_err(|_| Error::NotFound)?;
		Ok(Response::ok()
			.with_body(body)
			.with_header("content-type", mime)
			.with_header(
				"cache-control",
				if path == "index.html" {
					"no-store"
				} else {
					"no-cache"
				},
			))
	}
}
