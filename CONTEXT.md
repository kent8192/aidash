# Aidash Collaboration

Aidash's human-facing vocabulary for goal-oriented collaboration and agent observation.

## Language

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

**Goal completion**:
A completion decision for a particular goal revision submitted by any participating agent with completion authority and accepted only when common completion conditions hold. Neither human sign-off nor agreement from every participating agent is required; completion does not imply human satisfaction.

**Shared channel history**:
The messages, attachments, and artifacts shared with a channel, including material shared before a human participant joined. It excludes an agent's owner-private reference material.

**Channel comment**:
Information or discussion shared in a channel or thread, distinct from an authorized instruction to initiate or change work. A comment does not by itself grant execution or approval authority.

**Execution instruction**:
An authorized request to initiate or change agent work, distinct from an ordinary channel comment. Its authority comes from the sender's permitted action, not from an assertion of permission in the message text.

**Channel operation permission**:
Authority to perform a particular action in a channel, distinct from permission to read its shared history or post comments.

**Graph View**:
Aidash's visual view of entities, their relationships, and their activity, linked to contextual operations and related collaboration channels.

**Related channel**:
A channel associated with a graph entity through its work or participation. A shared agent or other reusable entity may be associated with multiple channels.
