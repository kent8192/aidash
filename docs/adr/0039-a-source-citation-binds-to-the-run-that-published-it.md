# A Source citation binds to the Run that published it

Decided on 2026-10-13 for #226. A final answer cites Evidence fragments with `[[cite:ev_<uuid>]]`, and the Harness verifies those references only against the publishing Run's own fragments. A Source citation is therefore that pair: a reference, in text a Run published, to an Evidence fragment of the same Run. The dashboard links only such references, and the evidence read API resolves a reference only within the Run that published the text. A reference to another Run's evidence, or one in text a person or another Run wrote, cites nothing, even when the reader may read the Run that holds the fragment.

## Considered Options

- **Resolve any reference by its evidence ID alone**, for whoever may read the Run that holds the fragment. This leaks nothing, but a person's message, an unverified answer or a parent Run relaying a child Run's text could then present a reference with the same standing as a verified citation.

## Consequences

- Each message and result Artifact records the Run that published it on the Node where the Run executed. Text another Node delivers records none, which costs nothing because Research operations are never exposed to remote Runs. The publishing path writes the Run from the authenticated Run, and the record is kept after that Run is deleted. People and peers cannot set it, and an idempotency key's format is never evidence of who published a text, because a peer chooses the keys of the messages it delivers.
- A parent Run that relays a child Run's answer does not carry the child's citations. Showing research across Runs needs its own design.
- A reader who cannot read the publishing Run gets the same answer as for a reference that does not exist.
