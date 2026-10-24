//! Deployment-wide sender schedule; `state` contains validated public identity,
//! source references and one proof, never private keys or Tenant data.
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "federation", table_name = "deployment_key_rollovers")]
#[derive(Serialize, Deserialize)]
pub struct DeploymentKeyRollover {
	#[field(primary_key = true, field_type = "text")]
	pub deployment: String,
	#[field(field_type = "text")]
	pub state: String,
}
