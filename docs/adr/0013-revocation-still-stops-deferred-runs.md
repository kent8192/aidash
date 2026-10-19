# Revoking any bound capability still stops a deferred Run

Under a deferred Exposure policy, revoking a Binding fails the step exactly as under the legacy policy, even when the model never loaded or discovered that capability. We rejected silently dropping unloaded revoked capabilities and notifying the model about loaded ones because that would change authority semantics, removing the pause visible to Deployment Administrators, inside an exposure change; any such relaxation needs its own decision (#115).
