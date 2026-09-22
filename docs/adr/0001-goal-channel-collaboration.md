# Goal-centered collaboration with contextual graph operations

Aidash's primary user experience consists of a Slack-style collaboration space and Graph View. Each channel corresponds to one goal; threads organize discussion independently of executable tasks, so a discussion can produce no tasks or several tasks. Necessary configuration belongs under Settings, while everyday work and intervention remain available from the primary views.

Graph View supports relationship exploration and authorized operations through contextual details, not editing execution structure by drawing connections. Navigation to collaboration follows the selected entity: a task or run opens its channel; a reusable entity prefers the current channel when related, otherwise offers the viewer's accessible related channels. An entity with no related channel stays in its details rather than creating a channel automatically.

An agent determines when a goal is complete; mandatory human acceptance is not part of that completion step. A human who finds the result insufficient can update the goal and continue the work. Agent completion and human satisfaction are therefore distinct concepts.

A human participant with channel read access can read the channel's shared history, including material shared before joining. This does not expose an agent's owner-private reference material or grant every operation: posting, execution, control, and approval remain subject to separately granted authority. Equivalent operations use the same authorization regardless of whether they originate in collaboration or Graph View.

These boundaries favor goal-oriented work without treating every conversation as an executable task, and preserve agent autonomy without making channel membership blanket execution authority. They require explicit links between discussions, work, and graph entities, as well as a defined execution boundary when a goal changes.
