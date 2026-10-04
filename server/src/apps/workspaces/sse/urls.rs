//! URL configuration for sse app (RESTful)

use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
}
