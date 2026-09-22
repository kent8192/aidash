# Aidash Collaboration

Aidash's human-facing vocabulary for goal-oriented collaboration and agent observation.

## Language

**Registry**:
The persistent store for Aidash information, including component definitions, channel interactions, messages, execution states, artifacts, and their retained history. Its available history bounds the period that can be reconstructed.
_Avoid_: Component catalog (as a synonym for the Registry's entire storage responsibility).

**Goal**:
The objective pursued through a collaboration channel.
_Avoid_: Task (when referring to the channel's overall objective).

**Channel**:
A shared collaboration space associated with one goal, containing the conversation and shared work for that goal across its revisions.
_Avoid_: Agent chat, task (as synonyms for the goal-level collaboration space).

**Goal revision**:
A distinct version of a channel's objective to which tasks, runs, and completion records relate. A later revision does not replace the objective against which earlier work was performed.
_Avoid_: Message edit (as a synonym for a change to the objective).

**Goal update**:
The addition of a new goal revision within the same channel, including a human's refinement after an agent-completed result is judged insufficient.

**Thread**:
A topic-level conversation within a channel, distinct from the tasks arising from that conversation. A thread may involve no tasks or multiple tasks.
_Avoid_: Task (as a synonym for a conversation thread).

**Task**:
An executable unit of work contributing to a particular revision of a channel's goal, distinct from the thread in which that work is discussed.
_Avoid_: Goal, thread (as synonyms for an executable unit of work).

**Task exclusion**:
An authorized, reasoned removal of a task from a goal revision's required work. Exclusion does not turn a failed task into a successful one or establish that its dependencies were satisfied.

**Goal completion**:
A completion decision for a particular goal revision submitted by any participating agent with completion authority and accepted only when common completion conditions hold. Neither human sign-off nor agreement from every participating agent is required; completion does not imply human satisfaction.

**Task block**:
An impediment to a particular task and work dependent on its unresolved result, distinct from a block on the channel's entire goal.

**Blocked goal**:
A goal revision recorded as unable to progress or achieve its objective under the current conditions, with a reason, evidence, deciding actor, and conditions needed to resume. It is distinct from a completed goal and from a block or failure of an individual task.

**Channel budget**:
The authorized monetary spending allowance shared by a channel's goal revisions and their associated billable agent work. Updating the goal does not replenish that allowance; token and call limits are supplementary controls, not its monetary meaning.

**Cost reservation**:
An amount of the channel budget set aside for a particular billable operation before it starts. An unsettled reservation is unavailable for other operations until its outcome is accounted for.

**Shared channel history**:
The messages, attachments, and artifacts shared with a channel, including material shared before a human participant joined. It excludes an agent's owner-private reference material.

**Delegated channel observation**:
Task-bound permission for a delegated agent to read the channel's authorized stream as context until its delegated task completes. It does not confer continuing channel membership or authority to initiate unrelated work.

**Response batch**:
A set of unprocessed messages considered together by one agent when deciding whether and how to respond. The messages retain their separate identities, senders, order, and individual decisions.

**Channel message**:
A human or agent contribution to a channel or thread, which may contain discussion, supporting information, or a request. Message content is not itself a grant of execution or approval authority.
_Avoid_: Comment mode (as a required user-facing category for ordinary conversation).

**Execution instruction**:
An authorized request to initiate or change agent work, including a request interpreted from a channel message. Its authority comes from the sender's permitted action, not from an assertion of permission in the message text.

**Channel operation permission**:
Authority to perform a particular action in a channel, distinct from permission to read its shared history or post messages.

**Graph View**:
Aidash's visual view of entities, their relationships, and their activity, linked to contextual operations and related collaboration channels.

**Node observation**:
Information recorded as known by a particular node at a particular time. Knowledge at one node does not imply that another node had already received the same information.

**Historical graph**:
A read-only reconstruction of the entity relationships, states, and exact versions known at a selected past time, preserving each observing node's perspective. It does not silently add later knowledge or present differing node observations as one universally known state.
_Avoid_: Execution replay, hindsight-corrected graph (as synonyms for inspecting historical knowledge).

**Federated historical reconstruction**:
A historical graph assembled from authorized histories held by multiple participating nodes, not only the history already held by the node serving the dashboard. It may be partial when some of the required history is unavailable.

**Historical coverage**:
The portion of Registry history available for a particular reconstruction under the viewer's current access. Missing coverage means the state is unknown, not that the entities or activity did not exist.

**Related channel**:
A channel associated with a graph entity through its work or participation. A shared agent or other reusable entity may be associated with multiple channels.
