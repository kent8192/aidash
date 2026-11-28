use super::*;
use aidash_domain::preview::{PreviewKind, PreviewLabel};

#[derive(Default)]
struct Fixture {
	domain: Option<String>,
	port: Option<u16>,
	origin: String,
	subject: String,
	credential: Uuid,
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
	async fn ticket(&mut self, ticket: TicketIssue) -> Result<()> {
		self.tickets.push(ticket);
		Ok(())
	}
	async fn live_preview(&mut self, live_preview_id: Uuid) -> Result<Preview> {
		Ok(self
			.preview
			.get_or_insert(Preview {
				id: Uuid::new_v4(),
				label: PreviewLabel::generate()?,
				kind: PreviewKind::Live { live_preview_id },
			})
			.clone())
	}
}

fn fixture() -> Fixture {
	Fixture {
		domain: Some("aidashusercontent.app".into()),
		port: None,
		origin: "https://dashboard.example".into(),
		subject: "alice".into(),
		credential: Uuid::new_v4(),
		preview: None,
		tickets: Vec::new(),
	}
}

#[tokio::test]
async fn a_deployment_without_a_preview_domain_reports_unavailable() {
	let mut fixture = fixture();
	fixture.domain = None;
	let error = issue_live(&mut fixture, Uuid::new_v4()).await.unwrap_err();
	assert_eq!(error.to_string(), "PREVIEW_UNAVAILABLE");
	assert!(fixture.tickets.is_empty());
}

#[tokio::test]
async fn a_ticket_url_names_the_preview_domain_and_hides_only_the_digest() {
	let mut fixture = fixture();
	let issued = issue_live(&mut fixture, Uuid::new_v4()).await.unwrap();
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
	let mut fixture = fixture();
	fixture.port = Some(8443);
	let issued = issue_live(&mut fixture, Uuid::new_v4()).await.unwrap();
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
