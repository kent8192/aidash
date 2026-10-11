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
		"Location: http://127.0.0.1/internal\r\n",
		"",
	)])
	.await;
	let mut reader = reader(address);
	reader.admit = |url| {
		if url.host_str() == Some("127.0.0.1") {
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
async fn names_resolving_to_private_addresses_are_refused_before_connecting() {
	let reader = PageReader {
		resolver: Box::new(Rebinding),
		admit: admit_public_url,
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
