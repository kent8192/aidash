# The Exposure policy is a code-defined version in the Agent definition

An Agent definition names a code-defined Exposure policy version (`legacy@1` when absent, or `deferred@1` with its budgets), which is copied into the Binding snapshot and fixed for the Run. We rejected a Registry kind or `BindingKind` for it because the policy has no Node provider and would add a catalog-approval surface for no authority benefit, and rejected reusing `BINDING_SCHEMA` because that versions the snapshot format, not behavior (#115).
