//! Research provider packages bound into Runs. The Node registers which
//! providers are available; every bound operation runs the same host-owned
//! research use case with this Node's evidence store and disclosure gate.
use crate::{
	Error, Result,
	domain::Run,
	provider::ToolSpec,
	tool::{Tool, ToolContext},
};
use aidash_application::{
	capabilities::approvals::DisclosureRequest,
	ports::research::{
		DisclosureGate, DisclosureState, PageProvider, PageRenderer, SearchProvider, WebDisclosure,
	},
	research::{Call, Research},
};
use aidash_domain::{
	research::{Failure, Operation, account::AccountProfile, evidence::Limits, failure},
	tool::{
		ToolContract,
		providers::{ToolDescriptor, research_contract, research_operation},
	},
};
use aidash_integrations::research::{
	brave::BraveSearch, fixture::FixtureResearch, page::PageReader, pdf::ChildEngine,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

/// Path of the Research account holder's Brave account profile (JSON).
pub const BRAVE_PROFILE_ENV: &str = "AIDASH_RESEARCH_BRAVE_PROFILE";
/// `enabled` admits the public page reading provider.
pub const PAGE_READING_ENV: &str = "AIDASH_RESEARCH_PAGE_READING";
/// `enabled` admits the deterministic fixture provider (tests and evaluation).
pub const FIXTURE_ENV: &str = "AIDASH_RESEARCH_FIXTURE";

/// Research providers this Node implements, read once from its environment.
pub struct Providers {
	brave: std::result::Result<Arc<BraveSearch>, String>,
	pages: Option<Arc<PageReader>>,
	/// Renders pages of PDFs that page reading retained, in a bounded child.
	renderer: Option<Arc<ChildEngine>>,
	fixture: Option<Arc<FixtureResearch>>,
	pub limits: Limits,
}

impl Providers {
	pub fn from_env() -> Self {
		let page_reading = std::env::var(PAGE_READING_ENV).as_deref() == Ok("enabled");
		Self {
			brave: brave_from_env(),
			pages: page_reading.then(|| Arc::new(PageReader::default())),
			// A binary that cannot name itself fails each render instead.
			renderer: page_reading.then(|| {
				Arc::new(ChildEngine::new(
					std::env::current_exe().unwrap_or_else(|_| "aidash".into()),
				))
			}),
			fixture: (std::env::var(FIXTURE_ENV).as_deref() == Ok("enabled"))
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
				Err(unavailable(&format!("requires {PAGE_READING_ENV}=enabled")))
			}
			"research.fixture@1" if self.fixture.is_none() => {
				Err(unavailable(&format!("requires {FIXTURE_ENV}=enabled")))
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

impl Providers {
	fn renderer(&self, provider: &str) -> Result<Arc<dyn PageRenderer>> {
		match provider {
			"research.page@1" => self
				.renderer
				.clone()
				.map(|renderer| renderer as Arc<dyn PageRenderer>)
				.ok_or_else(|| unavailable(provider)),
			"research.fixture@1" => self
				.fixture
				.clone()
				.map(|fixture| fixture as Arc<dyn PageRenderer>)
				.ok_or_else(|| unavailable(provider)),
			_ => Err(unavailable(provider)),
		}
	}
}

fn unavailable(provider: &str) -> Error {
	Error::Invalid(format!("PROVIDER_UNAVAILABLE: {provider}"))
}

fn brave_from_env() -> std::result::Result<Arc<BraveSearch>, String> {
	let path = match std::env::var(BRAVE_PROFILE_ENV) {
		Ok(path) if !path.is_empty() => path,
		_ => return Err(format!("requires {BRAVE_PROFILE_ENV}")),
	};
	let profile: AccountProfile = std::fs::read(&path)
		.ok()
		.and_then(|bytes| serde_json::from_slice(&bytes).ok())
		.ok_or_else(|| "has an unreadable account profile".to_owned())?;
	let token = crate::config::secret(&profile.credential_env)
		.map_err(|_| "has no configured tool secret".to_owned())?;
	BraveSearch::new(profile, &token)
		.map(Arc::new)
		.map_err(|_| "has unavailable account terms".to_owned())
}

/// The Node's Research evidence store over `store`.
pub fn evidence_store(
	store: crate::store::Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> impl aidash_application::ports::research::EvidenceStore {
	crate::apps::execution::repositories::research::Evidence { store, scope }
}

/// The Node's research providers. Configuration changes need a restart.
pub fn providers() -> &'static Providers {
	static PROVIDERS: OnceLock<Providers> = OnceLock::new();
	PROVIDERS.get_or_init(Providers::from_env)
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
			Operation::View => {
				// A viewed page reaches the model as a PNG image (ADR 0053); a
				// model without a current route for it would never see the
				// page the view records and counts.
				if !run
					.context
					.binding_snapshot
					.as_ref()
					.is_some_and(|snapshot| snapshot.model_reads_page_images())
				{
					return Ok(failure(Operation::View, Failure::ImageInputRequired));
				}
				research
					.view(&call, providers.renderer(&self.provider)?.as_ref(), &input)
					.await
			}
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
