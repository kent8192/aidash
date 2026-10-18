//! Research provider packages bound into Runs. The Node registers which
//! providers are available; every bound operation runs the same host-owned
//! research use case with this Node's evidence store and disclosure gate.
use crate::{
	Error, Result,
	apps::execution::serializers::research_providers::{BraveSettings, ResearchProviderSettings},
	domain::Run,
	provider::ToolSpec,
	tool::{Tool, ToolContext},
};
use aidash_application::{
	capabilities::approvals::DisclosureRequest,
	ports::research::{
		DisclosureGate, DisclosureState, PageProvider, SearchProvider, WebDisclosure,
	},
	research::{Call, Research},
};
use aidash_domain::{
	research::{Operation, evidence::Limits},
	tool::{
		ToolContract,
		providers::{ToolDescriptor, research_contract, research_operation},
	},
};
use aidash_integrations::research::{
	brave::BraveSearch, fixture::FixtureResearch, page::PageReader,
};
use async_trait::async_trait;
use reinhardt::conf::settings::profile::Profile;
use serde_json::Value;
use std::sync::{Arc, LazyLock, RwLock};
use uuid::Uuid;

/// Research providers this Node implements, built from the Deployment's
/// `[research_providers]` settings.
pub struct Providers {
	brave: std::result::Result<Arc<BraveSearch>, String>,
	pages: Option<Arc<PageReader>>,
	fixture: Option<Arc<FixtureResearch>>,
	pub limits: Limits,
}

impl Default for Providers {
	/// A Deployment that offers no Research provider.
	fn default() -> Self {
		Self {
			brave: Err("is not configured".into()),
			pages: None,
			fixture: None,
			limits: Limits::default(),
		}
	}
}

impl Providers {
	/// The providers `settings` declare. A Brave key whose Deployment secret
	/// is not set leaves Brave not set up rather than refusing to start.
	pub fn from_settings(settings: &ResearchProviderSettings) -> Self {
		Self {
			brave: settings
				.brave
				.as_ref()
				.ok_or_else(|| "is not configured".to_owned())
				.and_then(brave),
			pages: settings
				.page_reading
				.then(|| Arc::new(PageReader::default())),
			fixture: settings
				.fixture
				.then(|| Arc::new(FixtureResearch::default())),
			limits: Limits::default(),
		}
	}

	/// Whether `descriptor` names a research provider this Node can dispatch.
	pub fn available(&self, descriptor: &ToolDescriptor) -> aidash_application::Result<()> {
		let unavailable = |reason: &str| {
			aidash_application::Error::Invalid(format!(
				"PROVIDER_UNAVAILABLE: {} {reason}",
				descriptor.provider
			))
		};
		match descriptor.provider.as_str() {
			"research.brave@1" => self
				.brave
				.as_ref()
				.map(|_| ())
				.map_err(|reason| unavailable(reason)),
			"research.page@1" if self.pages.is_none() => {
				Err(unavailable("requires research_providers.page_reading"))
			}
			"research.fixture@1" if self.fixture.is_none() => {
				Err(unavailable("requires research_providers.fixture"))
			}
			"research.page@1" | "research.fixture@1" => Ok(()),
			_ => Err(unavailable("is not a research provider")),
		}
	}

	/// Every Research provider this Deployment knows, its operations and
	/// whether it is set up now. Account terms are checked on every call, so
	/// terms that lapsed after startup read as not set up. A Tenant's approved
	/// definitions are the caller's to fill in; why a provider is not set up
	/// is never said.
	pub fn availability(&self) -> crate::apps::execution::serializers::runs::ResearchProviders {
		use crate::apps::execution::serializers::runs::{
			ResearchProvider, ResearchProviderOperation, ResearchProviders,
		};
		let set_up = |provider: &str| match provider {
			"research.brave@1" => self
				.brave
				.as_ref()
				.is_ok_and(|brave| brave.account().is_ok()),
			"research.page@1" => self.pages.is_some(),
			"research.fixture@1" => self.fixture.is_some(),
			_ => false,
		};
		ResearchProviders {
			providers: aidash_domain::tool::providers::RESEARCH_PROVIDERS
				.iter()
				.map(|(provider, operations, _)| ResearchProvider {
					provider: (*provider).into(),
					set_up: set_up(provider),
					operations: operations
						.iter()
						.map(|operation| ResearchProviderOperation {
							operation: (*operation).into(),
							approved_definitions: vec![],
						})
						.collect(),
				})
				.collect(),
			limits: self.limits.into(),
		}
	}

	fn search(&self, provider: &str) -> Result<Arc<dyn SearchProvider>> {
		match provider {
			"research.brave@1" => self
				.brave
				.clone()
				.map(|brave| brave as Arc<dyn SearchProvider>)
				.map_err(|_| unavailable(provider)),
			"research.fixture@1" => self
				.fixture
				.clone()
				.map(|fixture| fixture as Arc<dyn SearchProvider>)
				.ok_or_else(|| unavailable(provider)),
			_ => Err(unavailable(provider)),
		}
	}

	fn pages(&self, provider: &str) -> Result<Arc<dyn PageProvider>> {
		match provider {
			"research.page@1" => self
				.pages
				.clone()
				.map(|pages| pages as Arc<dyn PageProvider>)
				.ok_or_else(|| unavailable(provider)),
			"research.fixture@1" => self
				.fixture
				.clone()
				.map(|fixture| fixture as Arc<dyn PageProvider>)
				.ok_or_else(|| unavailable(provider)),
			_ => Err(unavailable(provider)),
		}
	}
}

fn unavailable(provider: &str) -> Error {
	Error::Invalid(format!("PROVIDER_UNAVAILABLE: {provider}"))
}

fn brave(settings: &BraveSettings) -> std::result::Result<Arc<BraveSearch>, String> {
	let token = crate::config::secret(&settings.secret)
		.map_err(|_| "has no configured Deployment secret".to_owned())?;
	BraveSearch::new(settings.account.clone(), &token)
		.map(Arc::new)
		.map_err(|_| "has incomplete account terms".to_owned())
}

/// The Node's Research evidence store over `store`.
pub fn evidence_store(
	store: crate::store::Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> impl aidash_application::ports::research::EvidenceStore {
	crate::apps::execution::repositories::research::Evidence { store, scope }
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

/// Install the providers `settings` declare for this process. Startup calls
/// this before serving, after refusing a malformed declaration.
pub fn configure(settings: &ResearchProviderSettings, profile: &Profile) -> Result<()> {
	settings.check(profile)?;
	*PROVIDERS
		.write()
		.unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(Providers::from_settings(settings));
	Ok(())
}

/// The Node's research providers. Configuration changes need a restart.
pub fn providers() -> Arc<Providers> {
	PROVIDERS
		.read()
		.unwrap_or_else(|poisoned| poisoned.into_inner())
		.clone()
}

/// A bound research operation of one provider package.
pub(crate) struct ResearchTool {
	provider: String,
	operation: Operation,
}

impl ResearchTool {
	pub(crate) fn bind(descriptor: &ToolDescriptor) -> Option<Self> {
		let (operation, _) = research_operation(&descriptor.provider, &descriptor.operation)?;
		Some(Self {
			provider: descriptor.provider.clone(),
			operation,
		})
	}
}

/// Web disclosure approval through the worker's capability approvals.
struct Gate<'a> {
	context: &'a ToolContext,
}

#[async_trait]
impl DisclosureGate for Gate<'_> {
	async fn admit(
		&self,
		run: Uuid,
		disclosure: &WebDisclosure,
	) -> aidash_application::Result<DisclosureState> {
		// Without local authority there is no approval flow: fail closed.
		let Some(authority) = &self.context.home.authority else {
			return Ok(DisclosureState::Denied);
		};
		let request = DisclosureRequest {
			digest: disclosure.digest(run),
			operation: disclosure.operation,
			provider: disclosure.provider.clone(),
			destination: disclosure.outbound.destination.clone(),
			content: disclosure.outbound.content.clone(),
			context_revision: disclosure.context_revision,
			// The authority places the approval in the Run's Working Area.
			area: None,
			attribution: disclosure.attribution.clone(),
			query: disclosure.query.clone(),
		};
		authority
			.web_disclosure(&self.context.store, &self.context.run, &request)
			.await
			.map_err(Into::into)
	}
	async fn consume(&self, _: Uuid, approval_id: Uuid) -> aidash_application::Result<bool> {
		let Some(authority) = &self.context.home.authority else {
			return Ok(false);
		};
		authority
			.consume_web_disclosure(&self.context.store, &self.context.run, approval_id)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl Tool for ResearchTool {
	fn specification(&self) -> ToolSpec {
		aidash_application::research::specification(self.operation)
	}
	fn contract(&self) -> ToolContract {
		research_contract(self.operation)
	}
	fn replay_safe(&self) -> bool {
		self.contract().replay_safe()
	}
	async fn invoke(&self, context: &ToolContext, input: Value, key: &str) -> Result<Value> {
		let providers = providers();
		let store = crate::apps::execution::repositories::research::Evidence {
			store: context.store.clone(),
			scope: context.scope.clone(),
		};
		let gate = Gate { context };
		let research = Research {
			store: &store,
			gate: &gate,
			limits: providers.limits,
		};
		let run: &Run = &context.run;
		let call = Call {
			run: run.id,
			context_revision: run.observed_input_seq,
			key,
			provider: &self.provider,
			disclosure_approval: self.contract().behavior.disclosure_approval,
		};
		let result = match self.operation {
			Operation::Search => {
				research
					.search(&call, providers.search(&self.provider)?.as_ref(), &input)
					.await
			}
			Operation::Fetch => {
				research
					.fetch(&call, providers.pages(&self.provider)?.as_ref(), &input)
					.await
			}
			Operation::Find => research.find(&call, &input).await,
		};
		result.map_err(Into::into)
	}
}

/// One Research evidence retention pass in Deployment Scope, as the hourly
/// retention loop runs it under the configured Deployment periods.
pub async fn sweep_retention(store: &crate::store::Store) -> crate::Result<u64> {
	use crate::apps::execution::repositories::research::retention;
	Ok(retention::sweep(store, &store.research_retention).await?)
}
