//! A Home whose workspace embedding uses the Tenant's OpenRouter Provider
//! Credential (ADR 0006). The fake issuer stands in for the Credential Broker:
//! it records each mint and routes calls to the local embedding fixture. It
//! returns no bearer because the shared fixture rejects any Authorization
//! header; bearer delivery is covered by the broker and integration suites.
use aidash_application::{
	Result,
	provider_access::{Access, Context, TokenIssuer},
	provider_credentials::{
		KeyValidator, Service, Store, Validation,
		testing::{KeyMaterialWritePort as _, NoExchange},
	},
};
use aidash_domain::provider_credentials::{Provider, ProviderCredential};
use aidash_server::{
	apps::identity::repositories::provider_credentials::NativeRepository, federation::Federation,
};
use async_trait::async_trait;
use secrecy::SecretString;
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Default)]
struct MemoryStore(Mutex<BTreeMap<String, usize>>);
#[async_trait]
impl Store for MemoryStore {
	fn resource(&self, id: Uuid) -> String {
		format!("memory/{id}")
	}
	async fn create(&self, _: &str, id: Uuid) -> Result<()> {
		self.0.lock().unwrap().insert(self.resource(id), 0);
		Ok(())
	}
	async fn add_version(&self, _: &str, resource: &str, _: &SecretString) -> Result<String> {
		let mut versions = self.0.lock().unwrap();
		let count = versions.get_mut(resource).unwrap();
		*count += 1;
		Ok(format!("{resource}/versions/{count}"))
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
		let count = self.0.lock().unwrap().get(resource).copied().unwrap_or(0);
		Ok((1..=count)
			.map(|n| format!("{resource}/versions/{n}"))
			.collect())
	}
	async fn disable(&self, _: &str) -> Result<()> {
		Ok(())
	}
	async fn destroy(&self, _: &str) -> Result<()> {
		Ok(())
	}
	async fn delete(&self, resource: &str) -> Result<()> {
		self.0.lock().unwrap().remove(resource);
		Ok(())
	}
}
struct AcceptKeys;
#[async_trait]
impl KeyValidator for AcceptKeys {
	async fn validate(&self, _: Provider, _: &SecretString) -> Result<Validation> {
		Ok(Validation { warnings: vec![] })
	}
}
struct EndpointIssuer {
	endpoint: Arc<Mutex<String>>,
	mints: Arc<Mutex<Vec<(Context, Uuid)>>>,
}
#[async_trait]
impl TokenIssuer for EndpointIssuer {
	async fn mint(&self, context: &Context, row: &ProviderCredential) -> Result<Access> {
		row.require_active()?;
		self.mints.lock().unwrap().push((context.clone(), row.id));
		Ok(Access {
			endpoint: self.endpoint.lock().unwrap().clone(),
			bearer: String::new().into(),
			keyless: None,
		})
	}
}

/// What the test observes: where Tenant calls go and every mint.
#[derive(Clone)]
pub struct TenantEmbedding {
	pub endpoint: Arc<Mutex<String>>,
	pub mints: Arc<Mutex<Vec<(Context, Uuid)>>>,
	service: Arc<Service>,
}

/// Configure the Home before its application is built.
pub fn install(home: &mut Federation) -> TenantEmbedding {
	let service = Arc::new(Service {
		repository: Arc::new(NativeRepository {
			pool: home.store.control_pool.clone(),
			node: home.store.node_id.clone(),
		}),
		store: Arc::new(MemoryStore::default()),
		validator: Arc::new(AcceptKeys),
		exchange: Arc::new(NoExchange),
		fingerprint_key: "scoped-remote-fingerprint-key-of-32-bytes".into(),
		max_per_tenant: 5,
	});
	let embedding = TenantEmbedding {
		endpoint: Arc::new(Mutex::new(String::new())),
		mints: Arc::new(Mutex::new(Vec::new())),
		service: service.clone(),
	};
	home.registry = home.registry.clone().with_provider_credentials(true);
	home.store.provider_credentials = Some(service);
	home.store.capability_issuer = Some(Arc::new(EndpointIssuer {
		endpoint: embedding.endpoint.clone(),
		mints: embedding.mints.clone(),
	}));
	embedding
}

impl TenantEmbedding {
	/// Register and bind the Home Tenant's OpenRouter Provider Credential.
	pub async fn connect(&self, tenant: &str) -> Uuid {
		let id = self
			.service
			.create(
				tenant,
				Uuid::now_v7(),
				Provider::Openrouter,
				"scoped-home-provider-key".into(),
				"operator",
			)
			.await
			.unwrap()
			.provider_credential
			.id;
		self.service
			.bind(tenant, Provider::Openrouter, id, 0, "operator")
			.await
			.unwrap();
		id
	}
}
