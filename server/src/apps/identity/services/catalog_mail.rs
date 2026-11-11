//! SMTP adapter for Catalog Approval Request mail.
use aidash_application::{
	Result,
	ports::catalog_mail::{CatalogRequestMailer, CatalogRequestNotice},
};
use async_trait::async_trait;
use reinhardt::{
	conf::EmailSettings,
	mail::{EmailMessage, backend_from_settings},
};

/// An absent configuration is intentionally a no-op and is never a startup error.
pub(crate) struct CatalogMailer(pub(crate) Option<EmailSettings>);

impl CatalogMailer {
	pub(crate) fn configured(
		settings: Option<&crate::apps::operations::serializers::deployment::SmtpSettings>,
	) -> Self {
		let settings = settings
			.filter(|settings| !settings.host.trim().is_empty())
			.map(|settings| {
				let mut mail = EmailSettings::default();
				mail.backend = "smtp".into();
				mail.host = settings.host.clone();
				mail.port = settings.port;
				mail.from_email = settings.from.clone();
				mail.username = settings.username.clone();
				mail.password = settings.password.clone();
				mail.use_tls = settings.starttls;
				mail
			});
		Self(settings)
	}
}

#[async_trait]
impl CatalogRequestMailer for CatalogMailer {
	async fn send(&self, recipient: &str, notice: &CatalogRequestNotice) -> Result<()> {
		let Some(settings) = &self.0 else {
			return Ok(());
		};
		let backend = backend_from_settings(settings)
			.map_err(|error| aidash_application::Error::External(error.to_string()))?;
		let body = format!(
			"Definition: {}@{}\nNode: {}\nReason: {}\nReview: {}",
			notice.definition.id,
			notice.definition.version,
			notice.node,
			notice.reason,
			notice.review_link
		);
		let message = EmailMessage::builder()
			.subject("Catalog Approval Request")
			.body(body)
			.from(&settings.from_email)
			.to(vec![recipient.to_owned()])
			.build()
			.map_err(|error| aidash_application::Error::External(error.to_string()))?;
		backend
			.send_messages(&[message])
			.await
			.map_err(|error| aidash_application::Error::External(error.to_string()))?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn whitespace_smtp_host_is_a_noop() {
		let settings = crate::apps::operations::serializers::deployment::SmtpSettings {
			host: "  ".into(),
			port: 587,
			from: "not-an-address".into(),
			username: None,
			password: None,
			starttls: true,
		};
		let mailer = CatalogMailer::configured(Some(&settings));
		let notice = CatalogRequestNotice {
			definition: aidash_domain::registry::EntityRef {
				id: "example".into(),
				version: "1.0.0".into(),
			},
			node: aidash_domain::nodes::NodeName::derived("acme"),
			reason: "review".into(),
			review_link: "https://example.test".into(),
		};
		assert!(mailer.send("not-an-address", &notice).await.is_ok());
	}
}
