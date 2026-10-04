//! Compose the deployment use case through the process bootstrap.
use crate::config::settings::ProjectSettings;
use reinhardt::injectable;

pub use aidash_application::deployment::DeploymentObservations;
pub use aidash_domain::deployment::{
	Condition, Deployment, DeploymentEvent, DeploymentStatus, Pod, valid_label as label,
};

#[injectable(scope = "singleton")]
pub async fn provide_deployment(#[inject] settings: ProjectSettings) -> DeploymentObservations {
	crate::bootstrap::deployment_observations(&settings.kubernetes)
}
