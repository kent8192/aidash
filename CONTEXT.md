# Aidash Collaboration

Aidash's human-facing vocabulary for goal-oriented collaboration and agent observation.

## Language

**Goal**:
The objective pursued through a collaboration channel.
_Avoid_: Task (when referring to the channel's overall objective).

**Channel**:
A shared collaboration space associated with one goal, containing the conversation and shared work for that goal.
_Avoid_: Agent chat, task (as synonyms for the goal-level collaboration space).

**Thread**:
A topic-level conversation within a channel, distinct from the tasks arising from that conversation. A thread may involve no tasks or multiple tasks.
_Avoid_: Task (as a synonym for a conversation thread).

**Task**:
An executable unit of work contributing to a channel's goal, distinct from the thread in which that work is discussed.
_Avoid_: Goal, thread (as synonyms for an executable unit of work).

**Goal completion**:
An agent's determination that the channel's goal has been achieved, without requiring human sign-off. It does not imply that a human considers the result sufficient.

**Goal update**:
A change to the objective pursued through a channel, including a human's refinement after an agent-completed result is judged insufficient.

**Shared channel history**:
The messages, attachments, and artifacts shared with a channel, including material shared before a human participant joined. It excludes an agent's owner-private reference material.

**Channel operation permission**:
Authority to perform a particular action in a channel, distinct from permission to read its shared history.

**Graph View**:
Aidash's visual view of entities, their relationships, and their activity, linked to contextual operations and related collaboration channels.

**Related channel**:
A channel associated with a graph entity through its work or participation. A shared agent or other reusable entity may be associated with multiple channels.
