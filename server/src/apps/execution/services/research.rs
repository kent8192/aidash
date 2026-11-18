//! Page reading (#987): the `research.page@1` built-in, bound into Runs as an
//! ordinary tool. Each call reads the page again through Aidash's own page
//! reader; the harness has already obtained its Tool call approval.
use crate::{
	Error, Result,
	apps::execution::serializers::research_providers::ResearchProviderSettings,
	provider::ToolSpec,
	tool::{Tool, ToolContext},
};
use aidash_application::ports::research::PageProvider;
use aidash_domain::tool::{
	ToolContract,
	providers::{ToolDescriptor, research_contract, research_operation},
};
use aidash_integrations::research::page::PageReader;
use async_trait::async_trait;
use reinhardt::conf::settings::profile::Profile;
use serde_json::Value;
use std::sync::{Arc, LazyLock, RwLock};

/// Page reading as this Node offers it, built from the Deployment's
/// `[research_providers]` settings.
#[derive(Default)]
pub struct Providers {
	pages: Option<Arc<PageReader>>,
}

impl Providers {
	/// Page reading when `settings` turn it on.
	pub fn from_settings(settings: &ResearchProviderSettings) -> Self {
		Self {
			pages: settings
				.page_reading
				.then(|| Arc::new(PageReader::default())),
		}
	}

	/// Whether the descriptor's provider can be dispatched on this Node.
	pub fn available(&self, provider: &str) -> Result<()> {
		match provider {
			"research.page@1" if self.pages.is_none() => {
				Err(unavailable("requires research_providers.page_reading"))
			}
			"research.page@1" => Ok(()),
			_ => Err(unavailable("is not offered")),
		}
	}

	fn pages(&self, provider: &str) -> Result<Arc<dyn PageProvider>> {
		match provider {
			"research.page@1" => self
				.pages
				.clone()
				.map(|pages| pages as Arc<dyn PageProvider>)
				.ok_or_else(|| unavailable(provider)),
			_ => Err(unavailable(provider)),
		}
	}
}

fn unavailable(provider: &str) -> Error {
	Error::Invalid(format!("PROVIDER_UNAVAILABLE: {provider}"))
}

static PROVIDERS: LazyLock<RwLock<Arc<Providers>>> =
	LazyLock::new(|| RwLock::new(Arc::new(Providers::from_project_settings())));

impl Providers {
	/// The providers of the composed project settings, for a process that
	/// has not been configured by startup, such as a management command.
	fn from_project_settings() -> Self {
		crate::config::settings::get_settings()
			.ok()
			.and_then(|settings| settings.resolve().ok())
			.map(|settings| Self::from_settings(&settings.into_parts().0.research_providers))
			.unwrap_or_default()
	}
}

/// Install the page reading `settings` declare for this process. Startup
/// calls this before serving.
pub fn configure(settings: &ResearchProviderSettings, profile: &Profile) -> Result<()> {
	settings.check(profile)?;
	*PROVIDERS
		.write()
		.unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(Providers::from_settings(settings));
	Ok(())
}

/// The Node's page reading. Configuration changes need a restart.
pub fn providers() -> Arc<Providers> {
	PROVIDERS
		.read()
		.unwrap_or_else(|poisoned| poisoned.into_inner())
		.clone()
}

/// A bound page reading operation.
pub(crate) struct ResearchTool {
	provider: String,
}

impl ResearchTool {
	pub(crate) fn bind(descriptor: &ToolDescriptor) -> Option<Self> {
		research_operation(&descriptor.provider, &descriptor.operation)?;
		Some(Self {
			provider: descriptor.provider.clone(),
		})
	}
}

#[async_trait]
impl Tool for ResearchTool {
	fn specification(&self) -> ToolSpec {
		aidash_application::research::specification()
	}
	fn contract(&self) -> ToolContract {
		research_contract(aidash_domain::research::Operation::Fetch)
	}
	fn replay_safe(&self) -> bool {
		self.contract().replay_safe()
	}
	async fn invoke(&self, _context: &ToolContext, input: Value, _key: &str) -> Result<Value> {
		let pages = providers().pages(&self.provider)?;
		aidash_application::research::read(pages.as_ref(), &input)
			.await
			.map_err(Into::into)
	}
}
