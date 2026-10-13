use super::*;
use rstest::rstest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Resolves every name to the local fixture server.
struct Loopback(SocketAddr);
#[async_trait]
impl Resolver for Loopback {
	async fn resolve(&self, _: &str, _: u16) -> Result<Vec<SocketAddr>, Failure> {
		Ok(vec![self.0])
	}
}

/// A resolver answering with a private address, as DNS rebinding would.
struct Rebinding;
#[async_trait]
impl Resolver for Rebinding {
	async fn resolve(&self, _: &str, port: u16) -> Result<Vec<SocketAddr>, Failure> {
		let address = SocketAddr::from(([10, 0, 0, 7], port));
		if public_ip(address.ip()) {
			Ok(vec![address])
		} else {
			Err(Failure::NonPublicDestination)
		}
	}
}

/// Serves each scripted response to one connection, in order.
async fn serve(responses: Vec<String>) -> SocketAddr {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	tokio::spawn(async move {
		for response in responses {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut buffer = [0u8; 4096];
			let _ = socket.read(&mut buffer).await.unwrap();
			socket.write_all(response.as_bytes()).await.unwrap();
		}
	});
	address
}

fn http(status: &str, headers: &str, body: &str) -> String {
	format!(
		"HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
		body.len()
	)
}

fn reader(address: SocketAddr) -> PageReader {
	PageReader {
		resolver: Box::new(Loopback(address)),
		admit: |_| Ok(()),
		..PageReader::default()
	}
}

#[rstest]
#[tokio::test]
async fn html_pages_become_bounded_inert_lines() {
	let body = "<html><head><title>Example &amp; Co</title><style>p{}</style></head><body><p>Alpha line</p><script>alert('x')</script><p>Die Stra&szlig;e &lt;script&gt;inert text&lt;/script&gt; &#x41;</p><!-- hidden --></body></html>";
	let address = serve(vec![http(
		"200 OK",
		"Content-Type: text/html; charset=utf-8\r\n",
		body,
	)])
	.await;
	let page = reader(address)
		.fetch(&Url::parse("http://example.com/page").unwrap())
		.await
		.unwrap();
	assert_eq!(page.title.as_deref(), Some("Example & Co"));
	let texts = page
		.lines
		.iter()
		.map(|line| line.text.as_str())
		.collect::<Vec<_>>();
	assert_eq!(texts[0], "Alpha line");
	assert!(texts[1].starts_with("Die Stra&szlig;e <script>inert text</script> A"));
	assert!(
		!texts
			.iter()
			.any(|text| text.contains("alert") || text.contains("hidden"))
	);
	assert!(page.validate().is_ok());
	assert_eq!(page.completeness, Completeness::Complete);
}

#[rstest]
#[tokio::test]
async fn redirects_are_admitted_again_before_any_request() {
	let address = serve(vec![http(
		"302 Found",
		"Location: http://example.com:8443/internal\r\n",
		"",
	)])
	.await;
	let mut reader = reader(address);
	reader.admit = |url| {
		if url.port() == Some(8443) {
			Err(Failure::NonPublicDestination)
		} else {
			Ok(())
		}
	};
	let outcome = reader
		.fetch(&Url::parse("http://example.com/").unwrap())
		.await;
	assert_eq!(outcome, Err(Failure::NonPublicDestination));
}

#[rstest]
#[tokio::test]
async fn redirects_never_leave_the_approved_destination_host() {
	let address = serve(vec![http(
		"301 Moved Permanently",
		"Location: https://elsewhere.example.net/page\r\n",
		"",
	)])
	.await;
	assert_eq!(
		reader(address)
			.fetch(&Url::parse("http://example.com/").unwrap())
			.await,
		Err(Failure::CrossHostRedirect)
	);
}

#[rstest]
#[tokio::test]
async fn a_redirect_fragment_is_dropped_from_the_fetched_page_address() {
	let address = serve(vec![
		http("302 Found", "Location: /next#section\r\n", ""),
		http("200 OK", "Content-Type: text/plain\r\n", "Next page"),
	])
	.await;
	let page = reader(address)
		.fetch(&Url::parse("http://example.com/").unwrap())
		.await
		.unwrap();
	assert_eq!(page.url, "http://example.com/next");
	assert!(page.validate().is_ok());
}

#[rstest]
fn extraction_is_linear_in_unclosed_skipped_elements() {
	// Many unclosed skipped elements once made each one rescan the rest.
	let body = "<script>".repeat(200_000) + "visible";
	let started = std::time::Instant::now();
	let outcome = extract(
		&Url::parse("https://example.com/").unwrap(),
		"text/html",
		body.as_bytes(),
	);
	assert!(started.elapsed() < std::time::Duration::from_secs(5));
	assert_eq!(outcome, Err(Failure::ExtractionFailed));
}

#[rstest]
#[tokio::test]
async fn names_resolving_to_private_addresses_are_refused_before_connecting() {
	let reader = PageReader {
		resolver: Box::new(Rebinding),
		..PageReader::default()
	};
	assert_eq!(
		reader
			.fetch(&Url::parse("https://rebinding.example.com/").unwrap())
			.await,
		Err(Failure::NonPublicDestination)
	);
}

#[rstest]
#[case(
	"200 OK",
	"Content-Type: application/pdf\r\n",
	"%PDF",
	Failure::UnsupportedMedia
)]
#[case("404 Not Found", "", "missing", Failure::ProviderRequestRejected)]
#[case("503 Service Unavailable", "", "busy", Failure::ProviderUnavailable)]
#[case(
	"200 OK",
	"Content-Type: text/html\r\n",
	"<script>only()</script>",
	Failure::ExtractionFailed
)]
#[tokio::test]
async fn unusable_responses_are_typed_failures(
	#[case] status: &str,
	#[case] headers: &str,
	#[case] body: &str,
	#[case] expected: Failure,
) {
	let address = serve(vec![http(status, headers, body)]).await;
	assert_eq!(
		reader(address)
			.fetch(&Url::parse("http://example.com/").unwrap())
			.await,
		Err(expected)
	);
}

#[rstest]
fn oversized_text_is_marked_incomplete() {
	let body = format!("{}\n", "x".repeat(MAX_LINE_BYTES + 10)).repeat(2);
	let page = extract(
		&Url::parse("https://example.com/").unwrap(),
		"text/plain",
		body.as_bytes(),
	)
	.unwrap();
	assert_eq!(page.completeness, Completeness::TextLimit);
	assert!(
		page.lines
			.iter()
			.all(|line| line.text.len() <= MAX_LINE_BYTES)
	);
	assert!(page.validate().is_ok());
}

#[rstest]
#[case("https://example.com/a", "/b", Ok("https://example.com/b"))]
#[case(
	"http://example.com/a",
	"https://example.com/b",
	Ok("https://example.com/b")
)]
#[case(
	"http://example.com/a",
	"http://example.com/b",
	Ok("http://example.com/b")
)]
#[case(
	"https://example.com/a",
	"/next#section",
	Ok("https://example.com/next")
)]
#[case(
	"https://example.com/a",
	"http://example.com/b",
	Err(Failure::InsecureRedirect)
)]
#[case(
	"https://example.com/a",
	"https://elsewhere.example.net/",
	Err(Failure::CrossHostRedirect)
)]
fn redirects_stay_on_the_approved_host_without_downgrading(
	#[case] current: &str,
	#[case] location: &str,
	#[case] expected: Result<&str, Failure>,
) {
	assert_eq!(
		redirect_target(&Url::parse(current).unwrap(), location).map(String::from),
		expected.map(String::from)
	);
}

#[rstest]
#[tokio::test]
async fn the_production_resolver_refuses_names_of_non_public_addresses() {
	for host in ["localhost", "127.0.0.1", "::1"] {
		assert_eq!(
			PublicDns.resolve(host, 80).await,
			Err(Failure::NonPublicDestination),
			"{host}"
		);
	}
}

#[rstest]
#[tokio::test]
async fn responses_beyond_the_size_cap_are_refused() {
	let body = "x".repeat(MAX_RAW_BYTES + 1);
	let address = serve(vec![http("200 OK", "Content-Type: text/plain\r\n", &body)]).await;
	assert_eq!(
		reader(address)
			.fetch(&Url::parse("http://example.com/").unwrap())
			.await,
		Err(Failure::ResponseTooLarge)
	);
}

#[rstest]
#[tokio::test]
async fn a_whole_read_is_bounded_by_its_deadline() {
	// Accept the connection and never answer.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let silent = tokio::spawn(async move {
		let (socket, _) = listener.accept().await.unwrap();
		tokio::time::sleep(Duration::from_secs(30)).await;
		drop(socket);
	});
	let reader = PageReader {
		timeout: Duration::from_millis(300),
		..reader(address)
	};
	let started = std::time::Instant::now();
	assert_eq!(
		reader
			.fetch(&Url::parse("http://example.com/").unwrap())
			.await,
		Err(Failure::Timeout)
	);
	assert!(started.elapsed() < Duration::from_secs(5));
	silent.abort();
}
