//! Real SQL/File schedules with only the sender selection clock virtualized.
//! Token timestamps and the receiving HTTP adapter keep their real Utc clocks.
use aidash_application::federation::key::{
	DeploymentKey, DeploymentKeyPublication, DeploymentKeySource, DeploymentKeys,
	FederationSigning, ScheduledDeploymentKeys,
};
use aidash_integrations::federation::key::FileDeploymentKey;
use aidash_server::{
	apps::operations::serializers::deployment::DeploymentKeySettings, federation::Federation,
};
use async_trait::async_trait;
use std::{
	path::PathBuf,
	sync::{
		Arc, Mutex,
		atomic::{AtomicU64, Ordering},
	},
};

struct SenderClock {
	keys: Arc<ScheduledDeploymentKeys>,
	time: Arc<AtomicU64>,
	selected: Arc<Mutex<Vec<(String, bool)>>>,
}
#[async_trait]
impl DeploymentKeys for SenderClock {
	async fn signing(&self, now: u64) -> aidash_application::Result<FederationSigning> {
		let virtual_time = self.time.load(Ordering::Acquire);
		let signing = self
			.keys
			.signing(if virtual_time == 0 { now } else { virtual_time })
			.await?;
		self.selected.lock().unwrap().push((
			signing.key.key_id().to_string(),
			signing.transition.is_some(),
		));
		Ok(signing)
	}
	async fn publication(&self, now: u64) -> aidash_application::Result<DeploymentKeyPublication> {
		let virtual_time = self.time.load(Ordering::Acquire);
		self.keys
			.publication(if virtual_time == 0 { now } else { virtual_time })
			.await
	}
}

pub struct SenderSchedule {
	_directory: tempfile::TempDir,
	settings: DeploymentKeySettings,
	successor_path: PathBuf,
	successor_id: String,
	keys: Arc<ScheduledDeploymentKeys>,
	time: Arc<AtomicU64>,
	selected: Arc<Mutex<Vec<(String, bool)>>>,
	retires_at: Option<u64>,
}
impl SenderSchedule {
	pub async fn install(f: &mut Federation, initial: PathBuf) -> Self {
		let settings = serde_json::from_value(serde_json::json!({"file":initial})).unwrap();
		let directory = tempfile::tempdir().unwrap();
		let successor_path = directory.path().join("successor.pem");
		let successor = FileDeploymentKey::generate(&successor_path, false).unwrap();
		let keys = Self::load(f, &settings).await;
		let schedule = Self {
			_directory: directory,
			settings,
			successor_path,
			successor_id: successor.key_id().to_string(),
			keys,
			time: Arc::new(AtomicU64::new(0)),
			selected: Arc::new(Mutex::new(Vec::new())),
			retires_at: None,
		};
		schedule.attach(f);
		schedule
	}
	async fn load(
		f: &Federation,
		settings: &DeploymentKeySettings,
	) -> Arc<ScheduledDeploymentKeys> {
		aidash_server::bootstrap::configure_deployment_keys(
			Some(settings),
			std::path::Path::new("/"),
			f.config.deployment.clone(),
			f.store.pool.clone(),
		)
		.await
		.unwrap()
		.unwrap()
	}
	fn attach(&self, f: &mut Federation) {
		f.store.egress.keys = Some(Arc::new(SenderClock {
			keys: self.keys.clone(),
			time: self.time.clone(),
			selected: self.selected.clone(),
		}));
	}
	pub async fn activate(&mut self, f: &Federation) {
		let real = chrono::Utc::now().timestamp() as u64;
		// Explicit fixture publication time, never a stored-row rewrite or wait.
		let published = real - 86_400;
		let state = self
			.keys
			.rollover(
				f.config.deployment_key.as_ref().unwrap().key_id(),
				DeploymentKeySource::File(self.successor_path.clone()),
				published,
			)
			.await
			.unwrap();
		let schedule = state.schedule.unwrap();
		assert_eq!(schedule.activates_at.timestamp() as u64, real);
		self.retires_at = Some(schedule.retires_at.timestamp() as u64);
		eprintln!(
			"sender activation: deployment={}, real_http_time={real}, explicit_publication_time={published}, retirement={}",
			f.config.deployment,
			self.retires_at.unwrap()
		);
	}
	/// Rebuild loader/schedule over the newly connected production pool.
	pub async fn reload(&mut self, f: &mut Federation) {
		self.keys = Self::load(f, &self.settings).await;
		self.attach(f);
	}
	pub async fn retire_sender(&self, f: &Federation) {
		let virtual_time = self.retires_at.unwrap();
		self.time.store(virtual_time, Ordering::Release);
		let real = chrono::Utc::now().timestamp() as u64;
		let public = f
			.store
			.egress
			.keys
			.as_ref()
			.unwrap()
			.publication(real)
			.await
			.unwrap();
		assert_eq!(public.current.id, self.successor_id);
		assert!(public.retiring.is_none());
		assert!(public.transition.is_none());
		eprintln!(
			"virtual sender retirement: selection_time={virtual_time}, real_http_time={real}; receiver clock unchanged"
		);
	}
	pub fn assert_selected(&self, proof: bool) {
		assert!(
			self.selected
				.lock()
				.unwrap()
				.iter()
				.any(|(id, carried)| id == &self.successor_id && *carried == proof),
			"actual HTTP transport must select successor with transition={proof}"
		);
	}
	pub async fn assert_trusted_by(&self, receiver: &Federation, node: &str, source: &str) {
		use aidash_application::ports::federation::inbound::{
			InboundRepository, ReceivingKeyOwner,
		};
		let owner = ReceivingKeyOwner::Registration {
			tenant: aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			node: aidash_domain::nodes::NodeAddress::parse(node).unwrap().node,
			source_node: aidash_domain::nodes::NodeAddress::parse(source).unwrap(),
		};
		let current = receiver.receiving_keys(&owner).await.unwrap().unwrap();
		assert_eq!(current.keys.current.key_id.as_str(), self.successor_id);
		assert!(!current.keys.quarantined);
	}
}
