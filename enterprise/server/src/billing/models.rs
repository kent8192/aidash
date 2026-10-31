// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! The billing app's tables. All of them are Deployment-wide: a Billing
//! Account, an attachment and a metering record belong to no Tenant, name any
//! Tenant by its Tenant name, and outlive its purge.
use chrono::{DateTime, NaiveDate, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Who pays. One External Identity owns it; there is no organization entity
/// and ownership never transfers.
#[model(app_label = "billing", table_name = "billing_accounts")]
#[derive(Serialize, Deserialize)]
pub struct BillingAccount {
	#[field(primary_key = true)]
	pub id: Uuid,
	pub owner_identity: Uuid,
	#[field(field_type = "text")]
	pub label: String,
	pub created_at: DateTime<Utc>,
}

/// The Subscription of a Billing Account as Aidash last learned it. Admission
/// reads this row and never asks the payment processor.
#[model(app_label = "billing", table_name = "billing_subscriptions")]
#[derive(Serialize, Deserialize)]
pub struct BillingSubscription {
	#[field(primary_key = true)]
	pub account_id: Uuid,
	#[field(field_type = "text")]
	pub tier: String,
	/// `active`, `past_due`, `suspended` or `canceled`.
	#[field(field_type = "text")]
	pub status: String,
	/// When the dunning grace period ends, while the account is past due.
	#[field(null = true)]
	pub grace_ends_at: Option<DateTime<Utc>>,
	pub updated_at: DateTime<Utc>,
}

/// The Billing Account that currently covers a Tenant. A Tenant has at most one.
#[model(app_label = "billing", table_name = "billing_attachments")]
#[derive(Serialize, Deserialize)]
pub struct BillingAttachment {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	pub account_id: Uuid,
	pub attached_at: DateTime<Utc>,
	/// The External Identity that consented as a Tenant Administrator.
	pub consented_by_administrator: Uuid,
	/// The External Identity that consented as the receiving account's owner.
	pub consented_by_owner: Uuid,
}

/// One party's consent to attaching a Tenant to a Billing Account. The
/// attachment happens when both parties have consented.
#[model(app_label = "billing", table_name = "billing_attachment_consents")]
#[derive(Serialize, Deserialize)]
pub struct BillingAttachmentConsent {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub account_id: Uuid,
	/// `tenant_administrator` or `account_owner`.
	#[field(field_type = "text")]
	pub party: String,
	pub identity_id: Uuid,
	pub created_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
}

/// The time one Execution Pod ran for a Tenant, in milliseconds that were
/// metered: time after the Tenant's Deletion was recorded is not.
#[model(app_label = "billing", table_name = "billing_pod_usage")]
#[derive(Serialize, Deserialize)]
pub struct BillingPodUsage {
	/// The operation that ran in the Pod, so a repeated report is the same row.
	#[field(primary_key = true)]
	pub operation: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub started_at: DateTime<Utc>,
	pub ended_at: DateTime<Utc>,
	pub millis: i64,
}

/// The bytes of files stored for a Tenant, sampled once a UTC day.
#[model(app_label = "billing", table_name = "billing_storage_samples")]
#[derive(Serialize, Deserialize)]
pub struct BillingStorageSample {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub sampled_on: NaiveDate,
	pub sampled_at: DateTime<Utc>,
	pub bytes: i64,
}
