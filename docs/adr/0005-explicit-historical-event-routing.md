---
status: accepted
---

# Start live subscriptions at participation and request historical work explicitly

For [Issue #70](https://github.com/kent8192/aidash/issues/70), ordinary event subscriptions start with participation; a newly joined Agent may read authorized history as context, but considering earlier events as triggers requires an explicit request with a selected scope. Existing subscribers automatically recover their unfinished recipient work after interruption, preserving the distinction between an obligation already accepted and new work requested from history. This avoids unexpected historical actions and inference costs on enrollment while retaining an intentional way to ask a new participant to handle earlier events; historical routing never implies unconditional repetition of external effects.

[ADR-0007](0007-event-time-subscription-revisions.md) fixes live eligibility at event commit and records subscription changes at sequence boundaries. Re-enabling a disabled subscription applies to future events and does not silently revive deliveries that disablement prevented from reaching execution; those retain a visible disposition and require explicit reconsideration.

Historical operations distinguish retrying existing unfinished deliveries from routing a selected range to new recipients. Existing decisions and effect identities are reused, completed work is never reset, and deliberately doing completed work again requires a separately authorized new Task with an explicit relationship. Show the event range, recipient scope and counts before the operation and enforce current subscription-management and execution authority; [ADR-0008](0008-preserve-recipient-handler-identity-during-recovery.md) records the identity boundary.
