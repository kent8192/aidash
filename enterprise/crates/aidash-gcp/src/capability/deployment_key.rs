// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! The Deployment Key of Cloud and Dedicated: an Ed25519 key version in Cloud
//! KMS (ADR 0064). The private key never leaves KMS. The public key is fetched
//! once, so signing needs only the KMS call that Capability Tokens already use.
use super::{AccessTokenSource, KmsTokenSigner, bounded_json};
use aidash_application::federation::key::DeploymentKey;
use aidash_capability::{SignError, TokenSigner};
use aidash_domain::nodes::DeploymentKeyId;
use async_trait::async_trait;
use ed25519_dalek::{VerifyingKey, pkcs8::DecodePublicKey};
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::sync::Arc;

const ED25519: &str = "EC_SIGN_ED25519";

pub struct KmsDeploymentKey {
	signer: KmsTokenSigner,
	key_id: DeploymentKeyId,
	public_key_pem: String,
}

impl KmsDeploymentKey {
	/// `version` is `projects/.../cryptoKeyVersions/<n>`. A version that is not
	/// Ed25519, or whose public key arrives damaged, is refused.
	pub async fn new(
		version: String,
		tokens: Arc<dyn AccessTokenSource>,
	) -> Result<Self, SignError> {
		Self::fetch(KmsTokenSigner::new(version, tokens)?).await
	}

	async fn fetch(signer: KmsTokenSigner) -> Result<Self, SignError> {
		#[derive(Deserialize)]
		#[serde(rename_all = "camelCase")]
		struct PublicKey {
			name: String,
			pem: String,
			pem_crc32c: String,
			algorithm: String,
		}
		let token = signer.tokens.token().await?;
		let response = signer
			.client
			.get(format!("{}/{}/publicKey", signer.base, signer.kid))
			.bearer_auth(token.expose_secret())
			.send()
			.await
			.map_err(|_| SignError)?;
		let key: PublicKey = bounded_json(response, 16_384).await?;
		if key.name != signer.kid
			|| key.algorithm != ED25519
			|| key.pem_crc32c.parse::<u32>().ok() != Some(crc32c::crc32c(key.pem.as_bytes()))
		{
			return Err(SignError);
		}
		let public = VerifyingKey::from_public_key_pem(&key.pem).map_err(|_| SignError)?;
		Ok(Self {
			key_id: DeploymentKeyId::of_public_key(public.as_bytes()),
			public_key_pem: key.pem,
			signer,
		})
	}
}

#[async_trait]
impl DeploymentKey for KmsDeploymentKey {
	fn key_id(&self) -> &DeploymentKeyId {
		&self.key_id
	}
	fn public_key_pem(&self) -> &str {
		&self.public_key_pem
	}
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], SignError> {
		TokenSigner::sign(&self.signer, input).await
	}
}

#[cfg(test)]
mod tests;
