//! Authoritative sender schedule. Source adapters retain all private material.
use super::{DeploymentKey, Signing};
use crate::{Error, Result};
use aidash_capability::{
	federation::rollover::{
		RolloverClaims, TrustedRolloverKey, mint_transition, verify_transition,
	},
	public_key_bytes,
};
use aidash_domain::nodes::{
	DeploymentId, DeploymentKeyId,
	keys::{PublicDeploymentKey, RolloverSchedule},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
	path::{Component, PathBuf},
	sync::Arc,
};

/// Immutable key source reference, never key material. File paths are absolute
/// so a restarted worker and the operator process resolve the same source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum DeploymentKeySource {
	File(PathBuf),
	Kms(String),
}
impl DeploymentKeySource {
	pub fn validate(&self) -> Result<()> {
		let valid = match self {
			Self::File(path) => {
				path.is_absolute()
					&& path
						.to_str()
						.is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
					&& path
						.components()
						.all(|c| !matches!(c, Component::ParentDir | Component::CurDir))
			}
			Self::Kms(version) => {
				let parts: Vec<_> = version.split('/').collect();
				version.len() <= 1024
					&& parts.len() == 10
					&& parts[0] == "projects"
					&& parts[2] == "locations"
					&& parts[4] == "keyRings"
					&& parts[6] == "cryptoKeys"
					&& parts[8] == "cryptoKeyVersions"
					&& parts.iter().all(|p| {
						!p.is_empty()
							&& *p != "." && *p != ".."
							&& p.bytes()
								.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
					}) && parts[9].parse::<u64>().is_ok_and(|n| n > 0)
			}
		};
		if valid {
			Ok(())
		} else {
			Err(Error::Invalid(
				"invalid Deployment Key source reference".into(),
			))
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentPublicKey {
	pub id: String,
	pub public_key_pem: String,
}
impl DeploymentPublicKey {
	fn of(key: &dyn DeploymentKey) -> Result<Self> {
		let result = Self {
			id: key.key_id().to_string(),
			public_key_pem: key.public_key_pem().to_owned(),
		};
		result.validate()?;
		Ok(result)
	}
	fn validate(&self) -> Result<[u8; 32]> {
		let bytes = public_key_bytes(&self.public_key_pem)
			.map_err(|_| Error::Invalid("invalid Deployment Key public PEM".into()))?;
		PublicDeploymentKey::new(DeploymentKeyId::parse(&self.id)?, bytes)?;
		Ok(bytes)
	}
}

/// Exactly one public proof and schedule is retained, including after expiry,
/// so a later rollover cannot go backward. Replacing it is compare-and-set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentKeyRollover {
	pub deployment: DeploymentId,
	pub predecessor_source: DeploymentKeySource,
	pub predecessor: DeploymentPublicKey,
	pub successor_source: DeploymentKeySource,
	pub successor: DeploymentPublicKey,
	pub schedule: RolloverSchedule,
	pub transition: String,
}
impl DeploymentKeyRollover {
	pub fn validate(&self, deployment: &DeploymentId) -> Result<()> {
		if &self.deployment != deployment {
			return Err(Error::Invalid(
				"Deployment Key schedule belongs to another Deployment".into(),
			));
		}
		self.predecessor_source.validate()?;
		self.successor_source.validate()?;
		self.predecessor.validate()?;
		let successor = self.successor.validate()?;
		self.schedule.validate()?;
		let activation = timestamp(self.schedule.activates_at)?;
		let claims = verify_transition(
			&self.transition,
			&TrustedRolloverKey {
				kid: self.predecessor.id.clone(),
				public_key_pem: self.predecessor.public_key_pem.clone(),
			},
			deployment.as_ref(),
			activation,
		)
		.map_err(|_| Error::Invalid("invalid stored Deployment Key transition".into()))?;
		if claims.successor_kid != self.successor.id
			|| claims.successor_key_bytes().ok() != Some(successor)
			|| claims.published_at != timestamp(self.schedule.published_at)?
			|| claims.retires_at != timestamp(self.schedule.retires_at)?
		{
			return Err(Error::Invalid(
				"stored Deployment Key transition does not match its schedule".into(),
			));
		}
		Ok(())
	}
}

#[async_trait]
pub trait DeploymentKeyLoader: Send + Sync {
	async fn load(&self, source: &DeploymentKeySource) -> Result<Arc<dyn DeploymentKey>>;
}
#[async_trait]
pub trait DeploymentKeyStore: Send + Sync {
	async fn load(&self) -> Result<Option<DeploymentKeyRollover>>;
	/// Atomically replace this exact snapshot, or insert only if none exists.
	async fn compare_and_set(
		&self,
		expected: Option<&DeploymentKeyRollover>,
		next: &DeploymentKeyRollover,
	) -> Result<bool>;
}

pub struct FederationSigning {
	pub key: Arc<dyn DeploymentKey>,
	pub transition: Option<String>,
}
#[derive(Debug, Clone)]
pub struct DeploymentKeyPublication {
	pub current: DeploymentPublicKey,
	pub successor: Option<DeploymentPublicKey>,
	pub retiring: Option<DeploymentPublicKey>,
	pub schedule: Option<RolloverSchedule>,
	pub transition: Option<String>,
}
#[async_trait]
pub trait DeploymentKeys: Send + Sync {
	async fn signing(&self, now: u64) -> Result<FederationSigning>;
	async fn publication(&self, now: u64) -> Result<DeploymentKeyPublication>;
}

pub struct ScheduledDeploymentKeys {
	deployment: DeploymentId,
	initial: DeploymentKeySource,
	store: Arc<dyn DeploymentKeyStore>,
	loader: Arc<dyn DeploymentKeyLoader>,
}
impl ScheduledDeploymentKeys {
	pub fn new(
		deployment: DeploymentId,
		initial: DeploymentKeySource,
		store: Arc<dyn DeploymentKeyStore>,
		loader: Arc<dyn DeploymentKeyLoader>,
	) -> Self {
		Self {
			deployment,
			initial,
			store,
			loader,
		}
	}
	async fn state(&self) -> Result<Option<DeploymentKeyRollover>> {
		let state = self.store.load().await?;
		if let Some(state) = &state {
			state.validate(&self.deployment)?;
		}
		Ok(state)
	}
	async fn source(
		&self,
		source: &DeploymentKeySource,
		expected: Option<&DeploymentPublicKey>,
	) -> Result<Arc<dyn DeploymentKey>> {
		source.validate()?;
		let key = self.loader.load(source).await?;
		let public = DeploymentPublicKey::of(key.as_ref())?;
		if expected.is_some_and(|expected| expected.id != public.id) {
			return Err(Error::Invalid(
				"Deployment Key source no longer matches its published identity".into(),
			));
		}
		Ok(key)
	}
	/// Validate both source access and the expected current key before publishing.
	/// The CAS leaves at most one transition even across independent processes.
	pub async fn rollover(
		&self,
		expected: &DeploymentKeyId,
		successor_source: DeploymentKeySource,
		now: u64,
	) -> Result<DeploymentKeyPublication> {
		let time = datetime(now)?;
		let state = self.state().await?;
		if state.as_ref().is_some_and(|s| time < s.schedule.retires_at) {
			return Err(Error::Conflict(
				"a Deployment Key rollover is already in progress".into(),
			));
		}
		let (predecessor_source, predecessor_public) = match &state {
			Some(s) => (&s.successor_source, Some(&s.successor)),
			None => (&self.initial, None),
		};
		let predecessor = self.source(predecessor_source, predecessor_public).await?;
		if predecessor.key_id() != expected {
			return Err(Error::Conflict(
				"the current Deployment Key changed; read status again".into(),
			));
		}
		let successor = self.source(&successor_source, None).await?;
		if successor.key_id() == predecessor.key_id()
			|| state
				.as_ref()
				.is_some_and(|s| s.predecessor.id == successor.key_id().as_str())
		{
			return Err(Error::Invalid(
				"a rollover needs a new Deployment Key".into(),
			));
		}
		// A readable public key alone does not prove permission to sign in KMS.
		successor
			.sign(b"aidash Deployment Key source validation")
			.await
			.map_err(|_| Error::External("the successor Deployment Key could not sign".into()))?;
		let schedule = RolloverSchedule::new(time)?;
		let successor_public = DeploymentPublicKey::of(successor.as_ref())?;
		let claims = RolloverClaims {
			deployment: self.deployment.to_string(),
			predecessor_kid: predecessor.key_id().to_string(),
			successor_kid: successor_public.id.clone(),
			successor_public_key: URL_SAFE_NO_PAD.encode(successor_public.validate()?),
			published_at: now,
			activates_at: timestamp(schedule.activates_at)?,
			retires_at: timestamp(schedule.retires_at)?,
		};
		let transition = mint_transition(&claims, &Signing(predecessor.as_ref()))
			.await
			.map_err(|_| {
				Error::External("the current Deployment Key could not sign the transition".into())
			})?;
		let next = DeploymentKeyRollover {
			deployment: self.deployment.clone(),
			predecessor_source: predecessor_source.clone(),
			predecessor: DeploymentPublicKey::of(predecessor.as_ref())?,
			successor_source,
			successor: successor_public,
			schedule,
			transition,
		};
		next.validate(&self.deployment)?;
		if !self.store.compare_and_set(state.as_ref(), &next).await? {
			return Err(Error::Conflict(
				"the Deployment Key schedule changed; read status again".into(),
			));
		}
		self.publication(now).await
	}
}
#[async_trait]
impl DeploymentKeys for ScheduledDeploymentKeys {
	async fn signing(&self, now: u64) -> Result<FederationSigning> {
		let now = datetime(now)?;
		let state = self.state().await?;
		let Some(state) = state else {
			return Ok(FederationSigning {
				key: self.source(&self.initial, None).await?,
				transition: None,
			});
		};
		let active = now >= state.schedule.activates_at;
		let (source, public) = if active {
			(&state.successor_source, &state.successor)
		} else {
			(&state.predecessor_source, &state.predecessor)
		};
		Ok(FederationSigning {
			key: self.source(source, Some(public)).await?,
			transition: state.schedule.in_overlap(now).then_some(state.transition),
		})
	}
	async fn publication(&self, now: u64) -> Result<DeploymentKeyPublication> {
		let now = datetime(now)?;
		let state = self.state().await?;
		let Some(state) = state else {
			return Ok(DeploymentKeyPublication {
				current: DeploymentPublicKey::of(self.source(&self.initial, None).await?.as_ref())?,
				successor: None,
				retiring: None,
				schedule: None,
				transition: None,
			});
		};
		let pending = now < state.schedule.activates_at;
		let overlap = state.schedule.in_overlap(now);
		Ok(DeploymentKeyPublication {
			current: if pending {
				state.predecessor.clone()
			} else {
				state.successor.clone()
			},
			successor: pending.then_some(state.successor),
			retiring: overlap.then_some(state.predecessor),
			schedule: Some(state.schedule),
			transition: (pending || overlap).then_some(state.transition),
		})
	}
}
fn datetime(now: u64) -> Result<DateTime<Utc>> {
	i64::try_from(now)
		.ok()
		.and_then(|s| DateTime::from_timestamp(s, 0))
		.ok_or_else(|| Error::Invalid("invalid Deployment Key clock".into()))
}
fn timestamp(time: DateTime<Utc>) -> Result<u64> {
	if time.timestamp_subsec_nanos() != 0 {
		return Err(Error::Invalid(
			"Deployment Key schedules require whole seconds".into(),
		));
	}
	u64::try_from(time.timestamp())
		.map_err(|_| Error::Invalid("invalid Deployment Key schedule time".into()))
}
