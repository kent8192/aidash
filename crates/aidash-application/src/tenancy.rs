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

/// Why a transaction reaches every Tenant's data. The set is closed: work
/// that needs a new reason adds a variant here, where review sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeploymentScopeReason {
	/// Claiming, publishing, reconciling and recovering Run activations.
	ActivationClaim,
	/// Claiming and publishing the event outbox.
	OutboxDispatch,
	/// Claiming and delivering delegations that are due for a retry.
	DelegationRetry,
	/// Listing generation provisioning jobs.
	GenerationReconcile,
	/// Listing semantic entries that are due for indexing.
	SemanticIndexing,
	/// Semantic retention, engine jobs, memory decay and cleanup.
	SemanticMaintenance,
	/// Pruning inference progress, purging workbench and incident evidence,
	/// and display retention.
	Retention,
	/// Listing atomic transactions that need recovery.
	TransactionRecovery,
	/// Seeding built-in definitions into every Tenant's Registry.
	BuiltInSeeding,
	/// The `memory-recovery` command.
	MemoryRecovery,
	/// Recovering the credential store across Tenants.
	CredentialStoreRecovery,
	/// A Deployment Administrator's lists across Tenants.
	DeploymentAdministratorList,
	/// A Deployment Administrator's rows without a Tenant in Tenant-owned
	/// tables, such as a decision on an unaddressed Registration Request.
	DeploymentAdministration,
	/// Listing the Tenants an External Identity is mapped into.
	IdentityTenants,
	/// Resolving an inbound peer's local Tenant, until #241 gives every Node a
	/// Tenant.
	FederationInbound,
	/// The activity observer, which writes this label by hand.
	ActivityObserver,
	/// Test fixtures that span Tenants.
	#[cfg(feature = "test-support")]
	TestFixture,
}

impl DeploymentScopeReason {
	/// The stable label recorded as `application_name = deployment:<label>`.
	pub fn label(self) -> &'static str {
		match self {
			Self::ActivationClaim => "activation-claim",
			Self::OutboxDispatch => "outbox-dispatch",
			Self::DelegationRetry => "delegation-retry",
			Self::GenerationReconcile => "generation-reconcile",
			Self::SemanticIndexing => "semantic-indexing",
			Self::SemanticMaintenance => "semantic-maintenance",
			Self::Retention => "retention",
			Self::TransactionRecovery => "transaction-recovery",
			Self::BuiltInSeeding => "built-in-seeding",
			Self::MemoryRecovery => "memory-recovery",
			Self::CredentialStoreRecovery => "credential-store-recovery",
			Self::DeploymentAdministratorList => "deployment-administrator-list",
			Self::DeploymentAdministration => "deployment-administration",
			Self::IdentityTenants => "identity-tenants",
			Self::FederationInbound => "federation-inbound",
			Self::ActivityObserver => "activity-observer",
			#[cfg(feature = "test-support")]
			Self::TestFixture => "test-fixture",
		}
	}

	/// Whether work for this reason writes. A reason that only reads opens a
	/// `READ ONLY` transaction, which also refuses row locks.
	pub fn writes(self) -> bool {
		match self {
			Self::ActivationClaim
			| Self::OutboxDispatch
			| Self::DelegationRetry
			| Self::SemanticMaintenance
			| Self::Retention
			| Self::BuiltInSeeding
			| Self::MemoryRecovery
			| Self::CredentialStoreRecovery
			| Self::DeploymentAdministration => true,
			Self::GenerationReconcile
			| Self::SemanticIndexing
			| Self::TransactionRecovery
			| Self::DeploymentAdministratorList
			| Self::IdentityTenants
			| Self::FederationInbound
			| Self::ActivityObserver => false,
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
		assert!(TenancyScope::Deployment(DeploymentScopeReason::IdentityTenants).read_only());
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
			DeploymentScopeReason::BuiltInSeeding,
			DeploymentScopeReason::MemoryRecovery,
			DeploymentScopeReason::CredentialStoreRecovery,
			DeploymentScopeReason::DeploymentAdministratorList,
			DeploymentScopeReason::DeploymentAdministration,
			DeploymentScopeReason::IdentityTenants,
			DeploymentScopeReason::FederationInbound,
			DeploymentScopeReason::ActivityObserver,
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
