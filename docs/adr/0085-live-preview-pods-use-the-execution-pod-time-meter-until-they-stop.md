# Live preview Pods use the Execution Pod time meter until they stop

Cloud bills Live preview Execution Pods with the existing Pod-time meter from
start until stop, including time after the owning Run completes. Dedicated
records the same usage for reporting. A Run completing does not release the
resources of a still-running Live preview, so it does not end that usage.

Keep inference BYOK and the existing gateway-traffic decision boundary: Issue 310
settles whether gateway traffic is metered when its Cloud path is delivered.
This adds no separate Live preview meter or inference resale.

Decided in the 2026-10-23 design rounds for
[Issue 242](https://github.com/kent8192/aidash/issues/242). See the
[completion design](../decisions/issue-244-completion.md).
