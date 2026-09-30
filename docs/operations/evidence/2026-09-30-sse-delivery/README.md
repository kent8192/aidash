# SSE delivery acceptance evidence

See the [verification report](../../sse-delivery-results.md) for the tested workload,
thresholds, failed attempts and limitations. Reproduce with
`scripts/test-sse-delivery.sh --benchmark`.

- `manifest.json`: measured runtime/test hashes, binary hashes, baseline commit,
  environment, image identities and invocation settings.
- `measurements.json.gz`: all three active windows and six idle windows, including
  SQL counts/timings, causal metrics and CPU/RSS samples.
- `active-*-receipts.json.gz`: all 5,700 expected client receipts per active window.
- `functional.json`: independent-consumer, outage/replay and oversized-payload
  assertions from the separate-process cases.

Gzip files contain UTF-8 JSON. Fixture Node/Workspace IDs are synthetic. Native
binaries, source snapshots and full process logs remain in the local bundle
identified in the report. Measured runtime hashes can be checked against the
committed source. Documentation updates after measurement do not change those inputs.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| [active-1-receipts.json.gz](active-1-receipts.json.gz) | 93,684 | `37288bd0eaf81c28aa17b12c42499edad58e1fa7b5f1a3be6baf60d1e205dd5d` |
| [active-2-receipts.json.gz](active-2-receipts.json.gz) | 93,718 | `e91b71c9055465325c7551efd89d8a7944e2df8fd6f6a139982e29d19b27ffad` |
| [active-3-receipts.json.gz](active-3-receipts.json.gz) | 93,689 | `889bcd70d0417b1d58b43ecb5f7440b671181c4d6dc20bbbdf6c3fababa02969` |
| [functional.json](functional.json) | 706 | `d128478a0761b8e2a1a8fe73f2ac2a415e07a5592b9d45806d1e97abdaa094ef` |
| [manifest.json](manifest.json) | 14,971 | `981a0302c00d75c936e05ed44ebeda809ce81e0cd639de22e61a874f11f3127b` |
| [measurements.json.gz](measurements.json.gz) | 1,915,969 | `134c3694bc097d1af427719fc961aa2c6f50d8c002e02f9658ccdc4c96451557` |
