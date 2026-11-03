//! Publish the same authoritative public state that outbound signing selects.
use super::super::serializers::identity::DeploymentRolloverIdentity;
use crate::{Result, config::DeploymentIdentity, federation::Federation};
pub(crate) async fn publication(f: &Federation) -> Result<DeploymentIdentity> {
	let mut identity = f.config.deployment_identity();
	if let Some(keys) = &f.store.egress.keys {
		let public = keys
			.publication(chrono::Utc::now().timestamp().max(0) as u64)
			.await
			.map_err(|_| {
				crate::Error::External("Deployment Key publication is unavailable".into())
			})?;
		identity.key = Some(public.current.into());
		identity.key_rollover = public.schedule.map(|schedule| DeploymentRolloverIdentity {
			successor: public.successor.map(Into::into),
			retiring: public.retiring.map(Into::into),
			published_at: schedule.published_at.timestamp() as u64,
			activates_at: schedule.activates_at.timestamp() as u64,
			retires_at: schedule.retires_at.timestamp() as u64,
			transition: public.transition,
		});
	}
	Ok(identity)
}
