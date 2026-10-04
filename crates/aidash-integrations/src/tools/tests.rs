use super::*;
#[rstest::rstest]
#[tokio::test]
async fn web_fetch_retries_transient_statuses_and_reports_terminal_source_rejections() {
	use axum::{Router, extract::Path, http::StatusCode, routing::get};

	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(
			listener,
			Router::new().route(
				"/{status}",
				get(|Path(status): Path<u16>| async move {
					(StatusCode::from_u16(status).unwrap(), "source body")
				}),
			),
		)
		.await
		.unwrap();
	});
	let client = reqwest::Client::new();
	for status in [403, 404] {
		let url = reqwest::Url::parse(&format!("{endpoint}/{status}")).unwrap();
		let response = client.get(url.clone()).send().await.unwrap();
		assert_eq!(
			web_fetch_response(response, &url).await.unwrap(),
			json!({"ok":false,"error":{"kind":"http_status","url":url.as_str(),"status":status}})
		);
	}
	for status in [408, 429, 500] {
		let url = reqwest::Url::parse(&format!("{endpoint}/{status}")).unwrap();
		let response = client.get(url.clone()).send().await.unwrap();
		assert!(
			matches!(
				web_fetch_response(response, &url).await,
				Err(Error::External(_))
			),
			"HTTP {status} must remain on the worker retry path"
		);
	}
	let url = reqwest::Url::parse(&format!("{endpoint}/200")).unwrap();
	let response = client.get(url.clone()).send().await.unwrap();
	assert_eq!(
		web_fetch_response(response, &url).await.unwrap(),
		json!({"text":"source body"})
	);
	server.abort();
	let _ = server.await;
}
