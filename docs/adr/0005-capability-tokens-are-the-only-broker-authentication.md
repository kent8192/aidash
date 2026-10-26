# Capability Tokens are the only authentication to the Credential Broker

Decided on 2026-10-08 (#137). Each provider call made with a Provider Credential carries a Capability Token that the worker mints right after the `model.infer` policy decision. The token is signed with a Cloud KMS asymmetric key and expires about 60 seconds after issue. The Credential Broker checks it only when a request starts and keeps no per-token state. The broker accepts only internal ingress and runs with the Cloud Run invoker IAM check disabled, so the token signature is its only caller authentication.

## Considered Options

- Keep the Cloud Run invoker IAM check and send a Google ID token in `X-Serverless-Authorization`. Rejected: every Pod on the host can already obtain the VM identity, so the check adds little. It would also keep worker Pods on the metadata server and add a second header to every provider call.
- Sign with an Ed25519 key stored in the environment runtime secret. Rejected: a host compromise would copy the signing key and keep minting after eviction. With KMS, the key never leaves Google, every signature is audited, and disabling the key version stops minting.
- Enforce cumulative spend budgets and single-use tokens in the broker. Rejected for now: both need shared state across Cloud Run instances. Short expiry, per-call claims (model, maximum output tokens, mandatory ZDR) and Aidash's existing allowance accounting bound misuse instead.

## Consequences

- A compromised host can mint tokens while the compromise lasts but cannot read Key Material. Misuse is bounded by token expiry and claims, and is attributed through broker audit logs.
- A token can be replayed until it expires. Streaming may continue past expiry until the inference deadline.
- The VM runtime service account has no role on the broker and no access to BYOK secrets. Its only related permission is signing with the KMS key.
- The deploy pipeline (GitHub Actions through Workload Identity Federation) is part of the trust root for Key Material. Two paths remain open to it. The deploy service account holds project-wide `iam.serviceAccountAdmin` in the shared project, so it can grant itself token-creation rights on the broker service account. It also deploys the broker, so it can ship a modified broker. Decided on 2026-10-09: accept the pipeline as trust root for v1, and harden its entry point (protected GitHub Environment with required reviewers, and a WIF attribute condition on protected refs). Add an IAM deny policy that blocks the impersonation path, and alert on broker revision changes and BYOK data-access anomalies. Moving the broker into a separately deployed enclave is deferred, for Enterprise or for growth in key count.

## Implementation note (proposed by the #151 worker on 2026-10-11, filed by the batch lead; PR #210)

Implemented in #151: bootstrap tags broker service accounts and an IAM deny policy denies the deploy service account policy changes, token, signing, key creation, deletion, undeletion, disabling and OpenID-token permissions on them (ten permissions). Bootstrap-owned alerts cover broker redeploys through Cloud Run, Compute and GKE, broker IAM changes, BYOK-project audit-config changes, and Key Material reads by non-broker principals. `actAs` stays allowed (Cloud Run needs it), so broker redeploy remains the accepted residual path, detected by bootstrap-owned alerts. The broker image digest is reviewed code (`infra/gcp/environments/broker-image.json`). The `gcp-deploy` Environment is required by the WIF condition; a second reviewed Environment covers build runs (option C of the #151 reviewer question, adopted 2026-10-11). Live verification of the deny policy, tag targeting and alerts belongs to #152.

## Amendment (2026-10-26, #310)

- A Tenant Secret Capability Token is a second kind, with its own type and audience, that names one Tenant Secret at one version, a destination and its methods; the broker never accepts one kind as the other (ADR 0095).
- The server signs Tenant Secret Capability Tokens with a KMS key of its own, separate from the worker's, and the broker accepts both public keys.
