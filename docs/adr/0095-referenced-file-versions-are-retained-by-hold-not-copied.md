# Referenced File versions are retained by hold, not copied

Decided on 2026-10-26 while grilling File previews for the first useful task (#405, #489, #492, #499). A Working Area keeps only its current manifest, and each publication reclaims the File versions it replaced within seconds. A person who asks for a correction must still see the exact version they referred to. So a File version that something refers to becomes a Retained version: reclamation skips it until its last reference goes, its Working Area is cleaned up or its Tenant is deleted.

The references are a thread message that cites a File version (including a correction request) and a running Live preview. Referring to an HTML File version records the manifest of its Working Area at that revision and retains every File version in it, so that the page's relative references still resolve. Retained versions count toward the Tenant's quota, and a reference that would exceed it is refused with an explanation; the message can still be sent without it, and a Live preview does not start. Under Federation, the viewer's Deployment asks the hosting Node to retain the version on behalf of the delegated Run's subject and tells it when the reference goes. The hosting Node's hold expires after its Retention Rules period, so a lost release cannot leak storage.

## Considered Options

- **Copy the referenced version into a separate output object.** It stores every referenced file twice and adds a second download path. Rejected.
- **Retain every Run's final publication automatically.** Storage would grow with the number of Runs instead of with what people refer to. Rejected.
- **Retain nothing and show that the version is gone.** A correction could no longer show what it corrects, which #492 and #499 require. Rejected.
- **Retain only the HTML file, or only its directory.** Pages commonly refer to `../assets` and to files that later publications change. Rejected.
- **Copy the revision into a Live preview's Pod at start.** It copies the whole Working Area for each start and needs a second retention mechanism. Rejected.

## Consequences

- Download and File preview serve a Retained version as they serve a current one, after checking current authority over the Working Area.
- A message's reference is released when the message itself is removed, for example by Tenant Deletion; a person losing sight of the message does not release it, since others may still see it. A Live preview releases its revision when it stops.
- Retained bytes grow only with File versions that later publications replaced, as in version-control history.
- Cleaning up a Working Area or deleting a Tenant removes Retained versions too; a reference then shows the version as no longer retained instead of showing another version.
