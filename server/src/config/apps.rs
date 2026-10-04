//! Installed app registry for server.
//!
//! `reinhardt-admin startapp` automatically appends new apps here.

use reinhardt::installed_apps;

/// Migration ownership is limited to the applications installed by this project.
pub const APP_LABELS: [&str; 8] = [
	"identity",
	"registry",
	"workspaces",
	"execution",
	"federation",
	"knowledge",
	"marketplace",
	"operations",
];

installed_apps! {
	// Apps will be added here by `reinhardt-admin startapp`.
	identity: "identity",
	registry: "registry",
	workspaces: "workspaces",
	execution: "execution",
	federation: "federation",
	knowledge: "knowledge",
	marketplace: "marketplace",
	operations: "operations",
}
