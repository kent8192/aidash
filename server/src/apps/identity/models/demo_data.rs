//! Demo data (ADR 0079, ADR 0080): what the `demo-data` command loaded, the
//! Tenants it made, and the Demo Reservations it holds for people not yet
//! signed in.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

/// One loaded pair of inputs, Deployment-wide. The command writes it last, so
/// a row means every step finished and a rerun with the same digest does
/// nothing.
#[model(app_label = "identity", table_name = "demo_data_loads")]
#[derive(Serialize, Deserialize)]
pub struct DemoDataLoad {
	/// Hex SHA-256 of the canonical JSON of both inputs.
	#[field(primary_key = true, field_type = "text")]
	pub digest: String,
	#[field]
	pub loaded_at: DateTime<Utc>,
}

/// The Tenant a structure key was assigned. Deployment-wide and naming a
/// Tenant, so purge deletes it with the Tenant's data.
#[model(app_label = "identity", table_name = "demo_tenants")]
#[derive(Serialize, Deserialize)]
pub struct DemoTenant {
	/// The key in the structure input, stable across loads.
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field(field_type = "text")]
	pub tenant: String,
}

/// A Demo Reservation: the one path from a verified email address to a
/// Mapping. It names a Tenant and a fictional subject, and sign-in spends it.
#[model(app_label = "identity", table_name = "demo_reservations")]
#[derive(Serialize, Deserialize)]
pub struct DemoReservation {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	/// Lowercased.
	#[field(field_type = "text")]
	pub email: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub spent_at: Option<DateTime<Utc>>,
	/// The External Identity the reservation was spent on.
	#[field(null = true)]
	pub spent_by: Option<uuid::Uuid>,
}
