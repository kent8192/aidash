---
status: accepted
---

# Separate decision declarations from operator authority

Decider definitions and the DecisionGate contract are independent of a particular probability provider, with Jev supplying the first adapter. Mandatory authorization and safety rules belong to operator code, while action thresholds belong to the immutable Decider version and Agent or Node restrictions may only narrow permitted behavior. This separates reviewed decision policy from service implementation and prevents a probability estimate or catalog declaration from granting authority.
