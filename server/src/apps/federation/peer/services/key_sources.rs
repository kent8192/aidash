//! File/KMS source composition. Only immutable KMS versions are cached; files
//! are reopened so replacement or loss cannot silently change scheduled keys.
use aidash_application::{
	Result,
	federation::key::{DeploymentKey, DeploymentKeyLoader, DeploymentKeySource},
};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc};
#[derive(Default)]
pub(crate) struct Sources {
	kms: tokio::sync::Mutex<BTreeMap<String, Arc<dyn DeploymentKey>>>,
}
#[async_trait]
impl DeploymentKeyLoader for Sources {
	async fn load(&self, source: &DeploymentKeySource) -> Result<Arc<dyn DeploymentKey>> {
		source.validate()?;
		match source {
			DeploymentKeySource::File(_) => {
				aidash_integrations::federation::key::FileKeySources
					.load(source)
					.await
			}
			DeploymentKeySource::Kms(version) => {
				let mut keys = self.kms.lock().await;
				if let Some(key) = keys.get(version) {
					return Ok(key.clone());
				}
				let key = crate::bootstrap::build::current()
					.extension
					.deployment_key(version)
					.await?;
				keys.insert(version.clone(), key.clone());
				Ok(key)
			}
		}
	}
}
