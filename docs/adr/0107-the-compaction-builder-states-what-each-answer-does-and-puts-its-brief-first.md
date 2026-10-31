# The compaction builder states what each answer does and puts its brief first

Decided on 2026-10-30 for #633. `aidash.compaction/3` replaces `aidash.compaction/2`. It keeps what the
Decider may see (ADR 0042): the goal, conversation text, tool inputs, and results only by status and size.
It changes three texts:

- The rubric now describes the two questions and what each answer does. A kept call with a cut result
  still shows the first 300 characters, and the full result stays in the execution journal. An agent that
  forgets a call tends to repeat it, so a call stays unless it is plainly irrelevant to the goal. A full
  result stays only when the goal needs more than its head.
- The call question asks whether the agent should still see that the call was made. The result question
  asks whether the complete result must stay rather than only its first 300 characters.
- The rubric travels under the state key `brief`. serde_json sorts keys, so it now comes before `goal` and
  `history`. Under `rubric` it came last.

Measured on replayed #122 decisions with corpus labels, the held-out utility rose from 0.58 to 0.80 on Jev
at 0.5 and from 0.63 to 0.92 on Clef at 0.15, with no dropped correction on either Route
([evaluation](../evaluation/compaction-rubric-2026-10-30.md)). Route thresholds stay as they are. As in
ADR 0084, Aidash is undeployed: the built-in Decider keeps version 1.0.0 with the new builder, and
development data naming builder 2 must be recreated.

## Considered options

- Keep the texts and calibrate per-question thresholds: rejected. Separate thresholds for the call and the
  result scored about as well, but they need a new Route schema, and a Route threshold may not exceed 0.5.
- Adopt the best GEPA candidate: rejected. It led on the validation split only and lost on train and test.
- Show the Decider the head of each result: out of scope. It widens the disclosure ADR 0042 fixed and would
  need its own decision.
