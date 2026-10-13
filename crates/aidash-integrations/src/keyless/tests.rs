use super::*;
use secrecy::SecretString;
use std::time::Duration;

fn clients() -> Clients {
	Clients::new(
		reqwest::Client::new(),
		Duration::from_secs(5),
		Duration::from_secs(1),
	)
	.unwrap()
}

fn access(keyless: Option<Destination>) -> Access {
	Access {
		endpoint: "https://embeddings.example/v1".into(),
		bearer: SecretString::from(String::new()),
		keyless,
	}
}

#[test]
fn each_destination_selects_its_client() {
	let clients = clients();
	assert!(std::ptr::eq(
		clients.select(&access(None)).unwrap(),
		&clients.direct
	));
	assert!(std::ptr::eq(
		clients
			.select(&access(Some(Destination::Permitted)))
			.unwrap(),
		&clients.direct
	));
	assert!(std::ptr::eq(
		clients
			.select(&access(Some(Destination::PublicOnly)))
			.unwrap(),
		&clients.public
	));
	assert!(matches!(
		clients.select(&access(Some(Destination::Forbidden))),
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn a_name_resolving_to_loopback_is_refused_before_connecting() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let port = listener.local_addr().unwrap().port();
	let error = clients()
		.public
		.post(format!("http://localhost:{port}/v1/embeddings"))
		.send()
		.await
		.unwrap_err();
	assert!(matches!(send_error(error), Error::Forbidden));
	assert!(
		tokio::time::timeout(Duration::from_millis(200), listener.accept())
			.await
			.is_err(),
		"no connection may reach the loopback listener"
	);
}

#[tokio::test]
async fn ordinary_transport_failures_stay_external() {
	// An address literal skips the resolver; `provider_access` refuses private
	// ones before this client is chosen, so only the transport can fail here.
	let error = reqwest::Client::new()
		.get("http://127.0.0.1:9/")
		.timeout(Duration::from_secs(2))
		.send()
		.await
		.unwrap_err();
	assert!(matches!(send_error(error), Error::External(_)));
}
