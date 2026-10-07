# Aidash decision vocabulary

Aidash separates generation, decisions, and execution while coordinating durable Agent work. The existing [Registry capability glossary](docs/operations/registry-capability-glossary.md) defines the capability vocabulary.

## Language

**Decider**:
An immutable, versioned declaration of a decision task, its permitted evidence, questions, and action thresholds. A Decider expresses how a choice is evaluated independently of authority to perform the resulting action.
_Avoid_: Generation model, execution grant, compactor

**Decision provider**:
A service that supplies typed probability answers for a Decider's questions. It is distinct from the Decider declaration and from the operator that acts on the answers.
_Avoid_: Operator, Decider, generation model

**DecisionGate**:
The operator's decision boundary that combines mandatory rules, permitted decision evidence, and a Decider's answers to determine a branch. A DecisionGate is distinct from the provider that estimates probabilities.
_Avoid_: Model approval, inference response

**Decision hook**:
A fixed point in Agent work at which a particular kind of choice is evaluated. A hook is distinct from a model-selected invocation or a permission grant.
_Avoid_: Tool call, model routing

**Decision state**:
The permitted evidence presented for one decision evaluation. It is distinct from the Agent's complete reasoning context or the record retained after the decision.
_Avoid_: Full conversation, decision log

**Action threshold**:
A Decider's versioned criterion for selecting an allowed branch from validated answers. It is distinct from mandatory authority and safety rules.
_Avoid_: Permission, editable risk rank

**Decision evidence**:
The retained account of the exact declarations, candidates, validated answers, effective rules, selected branch, and result of a decision. Evidence is distinct from a default copy of the complete Decision state.
_Avoid_: Full-state archive, provider response body

**Branch replay**:
Re-evaluation of a recorded decision's branch using its retained validated answers and exact effective rules. It is distinct from asking the Decision provider to produce fresh answers.
_Avoid_: Model rerun, probability reproduction

**Enforce mode**:
A Decider's evaluation mode in which a permitted selected branch may change Agent work. Enforce is distinct from unconditional approval or guaranteed successful execution.
_Avoid_: Automatic permission, forced success

**Shadow mode**:
A Decider's evaluation mode that retains its proposed branch as evidence without applying that branch to Agent work. Shadow still represents a provider request when an evaluation is performed.
_Avoid_: Free simulation, implicit Enforce

**Decision attempt**:
One accounted request to a Decision provider for a set of questions. An attempt is distinct from a whole decision evaluation, which may require multiple requests.
_Avoid_: Question, completed decision

**Compaction candidate**:
A proposed reduced reasoning history produced by a decision evaluation. A candidate is distinct from the retained execution journal or an applied change to the Run.
_Avoid_: Deleted journal, rewritten source

**Decision outcome**:
An observed result linked to a recorded decision or its applied branch. An outcome is distinct from the probability estimate or proof that the decision caused later success.
_Avoid_: Confidence score, causal success label

**Decision allowance**:
An explicitly approved number of provider attempts available to a Run or generated lineage for an exact Decider. An allowance is distinct from a payload-size ceiling or permission to disclose information.
_Avoid_: Unlimited permission, question count limit

**Provider capability**:
The input shapes and operational constraints supported by a particular Decision provider version. A provider capability is distinct from an Aidash-wide fixed evaluation limit.
_Avoid_: Execution grant, universal decision ceiling
