//! Bounded request labels and response-body-owned pending metrics.
use futures_util::{Stream, StreamExt};
use reinhardt::urls::routers::PathPattern;
use reinhardt::{Method, Request, Response, http::response::StreamBody};
use std::{
	pin::Pin,
	sync::OnceLock,
	task::{Context, Poll},
	time::Instant,
};

struct RouteLabel {
	template: String,
	pattern: PathPattern,
	methods: Vec<Method>,
}

fn route_label(request: &Request) -> &'static str {
	static ROUTES: OnceLock<Vec<RouteLabel>> = OnceLock::new();
	let routes = ROUTES.get_or_init(|| {
		let mut routes: Vec<_> = crate::routes()
			.server_ref()
			.get_all_routes()
			.into_iter()
			.map(|(template, _, _, methods)| RouteLabel {
				pattern: PathPattern::new(template.clone()).expect("registered route pattern"),
				template,
				methods,
			})
			.collect();
		// Static endpoints take precedence over parameter routes and catch-alls.
		routes.sort_by_key(|route| {
			(
				route.template.contains("{<path:"),
				route.template.contains('{'),
			)
		});
		routes
	});
	let path = request.uri.path();
	let alternate = if path.ends_with('/') {
		path.trim_end_matches('/').to_owned()
	} else {
		format!("{path}/")
	};
	for candidate in [path, alternate.as_str()] {
		if let Some(route) = routes.iter().find(|route| {
			(route.methods.is_empty() || route.methods.contains(&request.method))
				&& route.pattern.is_match(candidate)
		}) {
			return &route.template;
		}
	}
	"unmatched"
}

fn method_label(method: &Method) -> &'static str {
	match *method {
		Method::GET => "GET",
		Method::POST => "POST",
		Method::PUT => "PUT",
		Method::PATCH => "PATCH",
		Method::DELETE => "DELETE",
		Method::HEAD => "HEAD",
		Method::OPTIONS => "OPTIONS",
		Method::CONNECT => "CONNECT",
		Method::TRACE => "TRACE",
		_ => "UNKNOWN",
	}
}

struct Pending(metrics::Gauge);
impl Drop for Pending {
	fn drop(&mut self) {
		self.0.decrement(1);
	}
}

pub(super) struct Observation {
	method: &'static str,
	route: &'static str,
	start: Instant,
	pending: Pending,
}
impl Observation {
	pub fn start(request: &Request) -> Self {
		let method = method_label(&request.method);
		let route = route_label(request);
		let gauge =
			metrics::gauge!("axum_http_requests_pending", "method" => method, "endpoint" => route);
		gauge.increment(1);
		Self {
			method,
			route,
			start: Instant::now(),
			pending: Pending(gauge),
		}
	}

	pub fn finish(self, mut response: Response, request_id: &str) -> Response {
		let status = response.status.as_u16().to_string();
		let elapsed = self.start.elapsed().as_secs_f64();
		// Retain the existing dashboard series while using Reinhardt's transport.
		metrics::counter!("axum_http_requests_total", "method" => self.method, "endpoint" => self.route, "status" => status.clone()).increment(1);
		metrics::histogram!("axum_http_requests_duration_seconds", "method" => self.method, "endpoint" => self.route, "status" => status.clone()).record(elapsed);
		metrics::counter!("aidash_http_requests_total", "method" => self.method, "route" => self.route, "status" => status).increment(1);
		metrics::histogram!("aidash_http_request_duration_seconds", "method" => self.method, "route" => self.route).record(elapsed);
		tracing::info!(
			request_id,
			method = self.method,
			route = self.route,
			status = response.status.as_u16(),
			duration_ms = elapsed * 1000.0,
			"HTTP response"
		);

		let content_length = response
			.headers
			.get(http::header::CONTENT_LENGTH)
			.cloned()
			.or_else(|| {
				// A native buffered body supplies an exact size hint. Keep its finite
				// framing when wrapping it as a stream for lifecycle ownership.
				(!response.is_streaming()
					&& !response.status.is_informational()
					&& !matches!(response.status.as_u16(), 204 | 304))
				.then(|| http::HeaderValue::from(response.body.len() as u64))
			});
		let body: StreamBody = if let Some(body) = response.take_stream_body() {
			body
		} else if let Some(file) = response.file_body().cloned() {
			Box::pin(async_stream::try_stream! {
				let mut offset = 0;
				while offset < file.len() {
					let source = file.clone();
					let chunk = tokio::task::spawn_blocking(move || source.read_chunk(offset, 64 * 1024)).await??;
					offset += chunk.len() as u64;
					yield chunk;
				}
			})
		} else {
			let bytes = std::mem::take(&mut response.body);
			Box::pin(futures_util::stream::once(async move { Ok(bytes) }))
		};
		response = response.with_stream(PendingBody {
			body,
			pending: Some(self.pending),
		});
		// with_stream removes Content-Length; retain explicit finite/range/HEAD framing.
		if let Some(length) = content_length {
			response
				.headers
				.insert(http::header::CONTENT_LENGTH, length);
		}
		response
	}
}

struct PendingBody {
	body: StreamBody,
	pending: Option<Pending>,
}
impl Stream for PendingBody {
	type Item = <StreamBody as Stream>::Item;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let result = self.body.poll_next_unpin(cx);
		if matches!(result, Poll::Ready(None | Some(Err(_)))) {
			self.pending.take();
		}
		result
	}
}

#[cfg(test)]
mod tests;
