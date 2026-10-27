//! Which Tenants' data a transaction may reach (#223, #263).
//!
//! Every transaction opens with exactly one scope, fixed when it begins.
//! Row-level policies admit only the scope's Tenant's rows, every row in a
//! Deployment Scope, and no Tenant-owned row in the no-Tenant scope.
use crate::Result;
use aidash_domain::tenancy::TenantName;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TenancyScope {
	/// Work within one Tenant, including a Deployment Administrator's work on it.
	Tenant(TenantName),
	/// Work that reaches the Tenant-owned data of every Tenant at once, for one
	/// declared reason.
	Deployment(DeploymentScopeReason),
	/// Work that touches only Deployment-wide data, such as signing in, and
	/// reaches no Tenant's data.
	NoTenant,
}

impl TenancyScope {
	/// The scope of one Tenant, validating its name.
	pub fn tenant(name: &str) -> Result<Self> {
		Ok(Self::Tenant(TenantName::new(name)?))
	}

	pub fn tenant_name(&self) -> Option<&TenantName> {
		match self {
			Self::Tenant(name) => Some(name),
			Self::Deployment(_) | Self::NoTenant => None,
		}
	}

	/// Whether the transaction must not write. Only a Deployment Scope whose
	/// reason only reads is read-only.
	pub fn read_only(&self) -> bool {
		matches!(self, Self::Deployment(reason) if !reason.writes())
	}
}

impl From<TenantName> for TenancyScope {
	fn from(name: TenantName) -> Self {
		Self::Tenant(name)
	}
}

impl From<DeploymentScopeReason> for TenancyScope {
	fn from(reason: DeploymentScopeReason) -> Self {
		Self::Deployment(reason)
	}
}

/// The scope of test fixtures that span Tenants. Tests that check isolation
/// open real Tenant scopes instead.
#[cfg(feature = "test-support")]
pub const TEST_FIXTURE: TenancyScope = TenancyScope::Deployment(DeploymentScopeReason::TestFixture);

/// Why a transaction reaches every Tenant's data. The set is closed: work
/// that needs a new reason adds a variant here, where review sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeploymentScopeReason {
	/// Claiming, publishing, reconciling and recovering Run activations, and
	/// the worker's lease bookkeeping for the Runs it claimed.
	ActivationClaim,
	/// Claiming independent Shadow approval observation jobs.
	ApprovalObservation,
	/// Claiming and publishing the event outbox.
	OutboxDispatch,
	/// Claiming and delivering delegations that are due for a retry.
	DelegationRetry,
	/// Listing and reconciling generation requests and foreign generation
	/// intents.
	GenerationReconcile,
	/// Listing semantic entries that are due for indexing.
	SemanticIndexing,
	/// Semantic retention, engine jobs, memory decay and cleanup.
	SemanticMaintenance,
	/// Pruning inference progress, purging workbench and incident evidence,
	/// display retention, and purging Research evidence and catching up the
	/// Runs whose evidence was revoked.
	Retention,
	/// Listing atomic transactions that need recovery.
	TransactionRecovery,
	/// Preparing, committing and rolling back an atomic transaction's
	/// mutations at a participant. The manifest names Tasks and Runs by id,
	/// not a Tenant, so no Tenant Scope can open the participant transaction
	/// until #265 records the Tenant on the Task and the Run.
	AtomicParticipant,
	/// Processing and reconciling capability operations, transferring
	/// capability records to peers, erasing and reclaiming capability work
	/// areas, and reaping Python heaps.
	CapabilityMaintenance,
	/// Seeding built-in definitions into every Tenant's Registry.
	BuiltInSeeding,
	/// Reading and registering definitions in the Deployment's single
	/// Registry, which every Tenant shares until #265 gives each its own.
	SharedRegistry,
	/// The `memory-recovery` command.
	MemoryRecovery,
	/// Recovering the credential store across Tenants.
	SecretStoreRecovery,
	/// Listing Provider Credentials whose stored Key Material needs reconciling.
	CredentialReconcile,
	/// A Deployment Administrator's lists across Tenants.
	DeploymentAdministratorList,
	/// A Deployment Administrator's rows without a Tenant in Tenant-owned
	/// tables, such as a decision on an unaddressed Registration Request.
	DeploymentAdministration,
	/// Reading the Mappings an External Identity holds in every Tenant, at
	/// sign-in, when switching Tenants and when it asks to join one. It writes
	/// only Deployment-wide sign-in and registration rows.
	IdentityTenants,
	/// Creating a Tenant: its root record, first policy and its Creator's
	/// Mapping. The Tenant's name is drawn inside the transaction (ADR 0050),
	/// so no Tenant Scope can name it when the transaction begins.
	TenantCreation,
	/// Revalidating External Identities that have work in any Tenant, and
	/// stopping a disabled identity's Runs in every Tenant.
	IdentityStatus,
	/// Resolving an inbound peer's local Tenant, until #241 gives every Node a
	/// Tenant.
	FederationInbound,
	/// Finding the Tenant of a Run, a Workspace or a remote operation from the
	/// row that records it, when the caller names only an id.
	RunTenant,
	/// Finding the owner, and so the Tenant, of a Plan Connection a Run's
	/// admission pinned by id, before the owner's Tenant Scope opens.
	PlanConnectionOwner,
	/// Reserving and consuming browser-bound OAuth attempts before their Tenant is known.
	OAuthAuthorization,
	/// The activity observer, which writes this label by hand.
	ActivityObserver,
	/// Writing one `marketplace.*` event for each audience Tenant of a package
	/// when its audience changes. It writes nothing else: an event belongs to
	/// one Tenant, and the writer's scope reaches only its own.
	MarketplaceDistribution,
	/// Counting the bytes of files each Tenant stores. It reads only.
	UsageSampling,
	/// Purging a deleted Tenant's data after its grace period, and deleting the
	/// audit history that outlived a purge (ADR 0061).
	TenantPurge,
	/// The `demo-data` command and the sign-in step that fulfils Demo
	/// Reservations. Each Tenant it fills is entered in that Tenant's own scope.
	DemoData,
	/// Test fixtures that span Tenants.
	#[cfg(feature = "test-support")]
	TestFixture,
}

impl DeploymentScopeReason {
	/// The stable label recorded as `application_name = deployment:<label>`.
	pub fn label(self) -> &'static str {
		match self {
			Self::ActivationClaim => "activation-claim",
			Self::ApprovalObservation => "approval-observation",
			Self::OutboxDispatch => "outbox-dispatch",
			Self::DelegationRetry => "delegation-retry",
			Self::GenerationReconcile => "generation-reconcile",
			Self::SemanticIndexing => "semantic-indexing",
			Self::SemanticMaintenance => "semantic-maintenance",
			Self::Retention => "retention",
			Self::TransactionRecovery => "transaction-recovery",
			Self::AtomicParticipant => "atomic-participant",
			Self::CapabilityMaintenance => "capability-maintenance",
			Self::BuiltInSeeding => "built-in-seeding",
			Self::SharedRegistry => "shared-registry",
			Self::MemoryRecovery => "memory-recovery",
			Self::SecretStoreRecovery => "secret-store-recovery",
			Self::CredentialReconcile => "credential-reconcile",
			Self::DeploymentAdministratorList => "deployment-administrator-list",
			Self::DeploymentAdministration => "deployment-administration",
			Self::IdentityTenants => "identity-tenants",
			Self::TenantCreation => "tenant-creation",
			Self::IdentityStatus => "identity-status",
			Self::FederationInbound => "federation-inbound",
			Self::RunTenant => "run-tenant",
			Self::PlanConnectionOwner => "plan-connection-owner",
			Self::OAuthAuthorization => "oauth-authorization",
			Self::ActivityObserver => "activity-observer",
			Self::MarketplaceDistribution => "marketplace-distribution",
			Self::UsageSampling => "usage-sampling",
			Self::TenantPurge => "tenant-purge",
			Self::DemoData => "demo-data",
			#[cfg(feature = "test-support")]
			Self::TestFixture => "test-fixture",
		}
	}

	/// Whether work for this reason writes. A reason that only reads opens a
	/// `READ ONLY` transaction, which also refuses row locks.
	pub fn writes(self) -> bool {
		match self {
			Self::ActivationClaim
			| Self::ApprovalObservation
			| Self::OutboxDispatch
			| Self::DelegationRetry
			| Self::SemanticMaintenance
			| Self::GenerationReconcile
			| Self::Retention
			| Self::CapabilityMaintenance
			| Self::BuiltInSeeding
			| Self::SharedRegistry
			| Self::MemoryRecovery
			| Self::SecretStoreRecovery
			| Self::DeploymentAdministration
			| Self::IdentityTenants
			| Self::TenantCreation
			| Self::IdentityStatus
			| Self::MarketplaceDistribution
			| Self::TenantPurge
			| Self::DemoData
			| Self::AtomicParticipant
			| Self::OAuthAuthorization => true,
			Self::SemanticIndexing
			| Self::TransactionRecovery
			| Self::CredentialReconcile
			| Self::DeploymentAdministratorList
			| Self::FederationInbound
			| Self::RunTenant
			| Self::PlanConnectionOwner
			| Self::ActivityObserver
			| Self::UsageSampling => false,
			#[cfg(feature = "test-support")]
			Self::TestFixture => true,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_reasons_that_read_are_read_only() {
		assert!(
			TenancyScope::Deployment(DeploymentScopeReason::DeploymentAdministratorList)
				.read_only()
		);
		assert!(!TenancyScope::Deployment(DeploymentScopeReason::Retention).read_only());
		assert!(!TenancyScope::tenant("acme").unwrap().read_only());
		assert!(!TenancyScope::NoTenant.read_only());
	}

	#[test]
	fn labels_are_kebab_case_and_distinct() {
		let reasons = [
			DeploymentScopeReason::ActivationClaim,
			DeploymentScopeReason::OutboxDispatch,
			DeploymentScopeReason::DelegationRetry,
			DeploymentScopeReason::GenerationReconcile,
			DeploymentScopeReason::SemanticIndexing,
			DeploymentScopeReason::SemanticMaintenance,
			DeploymentScopeReason::Retention,
			DeploymentScopeReason::TransactionRecovery,
			DeploymentScopeReason::AtomicParticipant,
			DeploymentScopeReason::CapabilityMaintenance,
			DeploymentScopeReason::BuiltInSeeding,
			DeploymentScopeReason::SharedRegistry,
			DeploymentScopeReason::MemoryRecovery,
			DeploymentScopeReason::SecretStoreRecovery,
			DeploymentScopeReason::CredentialReconcile,
			DeploymentScopeReason::DeploymentAdministratorList,
			DeploymentScopeReason::DeploymentAdministration,
			DeploymentScopeReason::IdentityTenants,
			DeploymentScopeReason::TenantCreation,
			DeploymentScopeReason::IdentityStatus,
			DeploymentScopeReason::FederationInbound,
			DeploymentScopeReason::RunTenant,
			DeploymentScopeReason::PlanConnectionOwner,
			DeploymentScopeReason::OAuthAuthorization,
			DeploymentScopeReason::ActivityObserver,
			DeploymentScopeReason::MarketplaceDistribution,
			DeploymentScopeReason::UsageSampling,
			DeploymentScopeReason::TenantPurge,
			DeploymentScopeReason::DemoData,
		];
		let labels = reasons
			.iter()
			.map(|reason| reason.label())
			.collect::<std::collections::BTreeSet<_>>();
		assert_eq!(labels.len(), reasons.len());
		for label in labels {
			assert!(
				label.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
				"{label}"
			);
		}
	}

	#[test]
	fn a_tenant_scope_validates_its_name() {
		assert!(TenancyScope::tenant("bad name").is_err());
		assert_eq!(
			TenancyScope::tenant("acme")
				.unwrap()
				.tenant_name()
				.unwrap()
				.as_str(),
			"acme"
		);
	}
}
