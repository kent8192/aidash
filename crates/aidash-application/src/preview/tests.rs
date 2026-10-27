use super::*;
use aidash_domain::preview::{PreviewKind, PreviewLabel};

#[derive(Default)]
struct Fixture {
	domain: Option<String>,
	port: Option<u16>,
	origin: String,
	subject: String,
	credential: Uuid,
	version: Option<FileVersion>,
	preview: Option<Preview>,
	tickets: Vec<TicketIssue>,
}

#[async_trait]
impl Scope for Fixture {
	fn preview_domain(&self) -> Option<&str> {
		self.domain.as_deref()
	}
	fn public_port(&self) -> Option<u16> {
		self.port
	}
	fn origin(&self) -> &str {
		&self.origin
	}
	fn subject(&self) -> &str {
		&self.subject
	}
	fn credential_id(&self) -> Uuid {
		self.credential
	}
	async fn authorize_file(&mut self, _: Uuid, _: Uuid) -> Result<FileVersion> {
		self.version.clone().ok_or(Error::NotFound("file".into()))
	}
	async fn preview(&mut self, kind: &PreviewKind) -> Result<Preview> {
		Ok(self
			.preview
			.get_or_insert(Preview {
				id: Uuid::new_v4(),
				label: PreviewLabel::generate()?,
				kind: kind.clone(),
			})
			.clone())
	}
	async fn ticket(&mut self, ticket: TicketIssue) -> Result<()> {
		self.tickets.push(ticket);
		Ok(())
	}
	async fn authorize_area(&mut self, _: Uuid) -> Result<()> {
		self.version
			.clone()
			.map(drop)
			.ok_or(Error::NotFound("file".into()))
	}
	async fn live_preview(&mut self, _: Uuid) -> Result<Preview> {
		Err(Error::NotFound("live preview".into()))
	}
}

fn fixture(media: &str) -> Fixture {
	Fixture {
		domain: Some("aidashusercontent.app".into()),
		port: None,
		origin: "https://dashboard.example".into(),
		subject: "alice".into(),
		credential: Uuid::new_v4(),
		version: Some(FileVersion {
			area_id: Uuid::new_v4(),
			file_id: Uuid::new_v4(),
			path: "index.html".into(),
			media_type: media.into(),
		}),
		preview: None,
		tickets: Vec::new(),
	}
}

#[tokio::test]
async fn a_deployment_without_a_preview_domain_reports_unavailable() {
	let mut fixture = fixture("text/html");
	fixture.domain = None;
	let error = issue(&mut fixture, Uuid::new_v4(), Uuid::new_v4())
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), "PREVIEW_UNAVAILABLE");
	assert!(fixture.tickets.is_empty());
}

#[tokio::test]
async fn only_html_and_pdf_versions_open_on_the_preview_domain() {
	let mut fixture = fixture("image/png");
	let error = issue(&mut fixture, Uuid::new_v4(), Uuid::new_v4())
		.await
		.unwrap_err();
	assert!(error.to_string().contains("HTML and PDF"));
}

#[tokio::test]
async fn a_ticket_url_names_the_preview_domain_and_hides_only_the_digest() {
	let mut fixture = fixture("text/html; charset=utf-8");
	let version = fixture.version.clone().unwrap();
	let issued = issue(&mut fixture, version.area_id, version.file_id)
		.await
		.unwrap();
	let ticket = fixture.tickets.pop().unwrap();
	assert!(issued.url.starts_with("https://"));
	assert!(
		issued
			.url
			.contains(".aidashusercontent.app/__aidash/ticket?t=")
	);
	assert!(!issued.url.contains(&ticket.digest));
	assert_eq!(ticket.subject, "alice");
	assert_eq!(ticket.credential_id, fixture.credential);
	assert_eq!(ticket.origin, "https://dashboard.example");
	assert_eq!(ticket.digest.len(), 64);
	assert!(ticket.expires_at > Utc::now());
}

#[tokio::test]
async fn a_public_port_is_carried_into_the_ticket_url() {
	let mut fixture = fixture("application/pdf");
	fixture.port = Some(8443);
	let version = fixture.version.clone().unwrap();
	let issued = issue(&mut fixture, version.area_id, version.file_id)
		.await
		.unwrap();
	assert!(
		issued
			.url
			.contains(".aidashusercontent.app:8443/__aidash/ticket?t=")
	);
}

#[test]
fn availability_reflects_the_configured_domain() {
	assert_eq!(
		Availability::of(Some("localhost")),
		Availability {
			available: true,
			preview_domain: Some("localhost".into()),
		}
	);
	assert!(!Availability::of(None).available);
}

#[test]
fn media_types_strip_parameters() {
	assert_eq!(media_type("text/html; charset=utf-8"), "text/html");
	assert_eq!(media_type("application/pdf"), "application/pdf");
}
