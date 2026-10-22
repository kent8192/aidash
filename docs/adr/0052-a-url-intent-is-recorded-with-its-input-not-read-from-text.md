# A URL intent is recorded with its input, not read from text

Decided on 2026-10-15 for #228. When a person asks a Run to read pages, the host records the addresses as `read_pages` beside the person's text on the input they arrive with: the Task for starting or queueing work, the Run input for steering. The stored text stays what the person wrote. The fixed instruction that asks the model to read the pages is never stored: it is rendered into a request's step state whenever the Task or a run message in that step carries `read_pages`, the same way the Task and run messages themselves are rebuilt into every step. The Thread draws a URL intent from the record, never from the text, so typing the instruction's wording yourself shows nothing more than text. This follows ADR 0039: a reference that appears in text a person wrote is not, by itself, the fact it looks like.

## Considered Options

- **Append the instruction to the stored text and recognize it by its heading line.** No schema change, but anyone could type the heading and make plain text look like a URL intent, rewording the instruction later would stop old messages from rendering as URL intents, and the instruction would appear wherever the text is shown, such as in Task lists. Rejected.

## Consequences

- URL intents live in their own `url_intents` records, keyed by the Run and, for a run input, its idempotency key (none for the Run's Task). Only the host writes them, from a validated request; people and peers cannot set them otherwise, and the Task and run input keep their existing shape.
- Rewording the instruction changes what every Run sees from its next step, because nothing stores the rendered text.
- The instruction is added only to steps whose Task or run messages carry `read_pages`, so other requests keep their exact bytes.
