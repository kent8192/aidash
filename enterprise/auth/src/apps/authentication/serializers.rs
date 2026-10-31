// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! HTTP DTOs. Tokens and passwords are never part of a stored page model.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
	SignIn,
	ChooseAccount,
	ProveExisting,
	ProveCurrent,
	ProveTarget,
	ProveOther,
}
impl Phase {
	pub fn title(self) -> &'static str {
		match self {
			Self::SignIn => "Sign in to Aidash",
			Self::ChooseAccount => "Choose your Aidash Account",
			Self::ProveExisting => "Verify your existing Account",
			Self::ProveCurrent => "Verify your current Account",
			Self::ProveTarget => "Verify the new sign-in method",
			Self::ProveOther => "Verify another sign-in method",
		}
	}
	pub fn description(self) -> &'static str {
		match self {
			Self::SignIn => "Use a configured sign-in method to continue to your product.",
			Self::ChooseAccount => {
				"Your sign-in method is verified. Choose whether to create a common Account or attach it to an Account you already use."
			}
			Self::ProveExisting => {
				"Sign in afresh with a method already linked to your existing Aidash Account. Matching email addresses do not link Accounts."
			}
			Self::ProveCurrent => {
				"Sign in afresh to prove the Aidash Account you are managing. Your existing SSO session is not sufficient for this change."
			}
			Self::ProveTarget => {
				"Now sign in afresh with the method you want to add. A method previously associated with another Account cannot be transferred."
			}
			Self::ProveOther => {
				"Sign in afresh with a different enabled method on this Account. The method being removed cannot prove this change."
			}
		}
	}
}
#[derive(Clone, Deserialize, Serialize)]
pub struct LoginContext {
	pub csrf: String,
	pub tenant_ids: Vec<String>,
	pub selected_tenant_id: String,
	pub providers: Vec<String>,
	pub password_sign_up: bool,
	pub mfa_mandatory: bool,
	pub client_id: String,
	pub phase: Phase,
	pub relink_only: bool,
	pub firebase: Firebase,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Firebase {
	pub project_id: String,
	pub api_key: String,
	pub auth_domain: String,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Account {
	pub account_id: String,
	pub source_identity_id: String,
	pub csrf: String,
	pub identities: Vec<Identity>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Identity {
	pub id: String,
	pub issuer: String,
	pub subject: String,
	pub linked: bool,
}
#[derive(Deserialize, Serialize)]
pub struct Next {
	pub redirect_to: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Enrollment {
	pub secret: String,
	pub uri: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SignOut {
	pub completed: bool,
}
