//! Whether this Deployment may load Demo data (ADR 0080).
use reinhardt::settings;

/// `[demo_data]` with `enabled = true`. Off by default: the `demo-data`
/// command refuses to run without it, and a sign-in fulfils Demo Reservations
/// only with it. Only the migration Job of a non-production Environment
/// turns it on.
#[settings(fragment = true, section = "demo_data")]
pub struct DemoDataSettings {
	#[setting(default = "false")]
	pub enabled: bool,
}
