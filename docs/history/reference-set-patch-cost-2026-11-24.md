# Reference set cost for patch publication

Issue #940 sets a budget of 10% additional end-to-end patch latency at 256
Reference files compared with one. On the release build, the first run exceeded
the budget by 34.32 ms and the confirmation run stayed within it. Publication
consistently added about 14 ms.

## Method

- Base revision `edcb95a6ba7fe71e90133109b331c8c11f37bbd5`. The two cases of
  `every_published_revision_holds_one_row_per_file_of_its_manifest` bind 1 or
  256 small UTF-8 Reference files to one Source. After Run admission and Working
  Area initialization, each case sends five sequential `apply_patch` HTTP
  requests that each add one distinct working file, using a fresh idempotency key.
- Local arm64 macOS 26.6.2, Rust 1.96.0, Testcontainers PostgreSQL 17, one test
  thread, and four Cargo build jobs. The build uses the release profile with
  Thin LTO. The local measurement diff exposes the existing fixture-only
  database helpers in release and skips CLI binaries that the selected
  test does not invoke.
- Local monotonic timers bracket each stage. The Run visibility row uses
  `run_for_reads` in `run_for_action`, which applies the local
  `run_for_interaction` visibility checks. The admission row includes its
  internal settings resolution and the Working Area load. Publication includes
  both `record_revision` calls.
- The top-level timer covers the full in-process HTTP request and response
  decoding. Upload, admission of the Run, and fixture setup happen before it.
  Medians are calculated independently from the five samples for each row.
  Other request work is the per-sample total less the timed top-level stages.
- The patch exercises the publication path shared with shell output described
  in the [November 13 review of #789](https://github.com/kent8192/aidash/issues/789#issuecomment-6845334848).
  The sample set exercises #940's relative patch budget. #719's broader
  acceptance covers Run start, command latency against a no-Reference baseline,
  and search on a 90-document set.

```bash
scripts/build-test-postgres.sh
RUST_TEST_THREADS=1 RUST_MIN_STACK=8388608 cargo test --release --locked \
  -p aidash-server --test core_capabilities \
  every_published_revision_holds_one_row_per_file_of_its_manifest -- --nocapture
```

The command above assumes the local instrumentation and fixture gates described
above. Use the disposable admission profile from `scripts/test-rust.sh`.

## Results

### First run

| Step                                       | 1 file (ms) | 256 files (ms) | Extra (ms) |
| ------------------------------------------ | ----------: | -------------: | ---------: |
| Initial Run read and authority inheritance |        4.45 |           5.79 |      +1.34 |
| Run visibility                             |       26.63 |          27.89 |      +1.26 |
| Use Recheck                                |      131.89 |         151.62 |     +19.73 |
| Endpoint settings                          |        1.75 |           2.85 |      +1.10 |
| Area initialization check                  |        0.36 |           0.35 |      -0.01 |
| Tool admission and Area load               |        3.12 |           5.52 |      +2.41 |
| Publication                                |        3.59 |          17.51 |     +13.92 |
| Previous record_revision                   |        0.62 |           2.58 |      +1.96 |
| New record_revision                        |        1.23 |          10.66 |      +9.44 |
| Other request work                         |       69.32 |          55.82 |     -13.49 |
| Total                                      |      246.33 |         305.29 |     +58.96 |

The total increased by 23.93%. The budget was 24.63 ms.

End-to-end samples in order (ms)

- 1 file: `[246.331, 261.66, 249.459, 236.562, 240.544]`
- 256 files: `[250.506, 317.866, 274.426, 305.287, 312.169]`

```text
Before tests  1:51  up  5:10, 1 user, load averages: 7.76 8.25 8.19
After tests   1:51  up  5:10, 1 user, load averages: 8.75 8.52 8.29
```

### Confirmation run

| Step                                       | 1 file (ms) | 256 files (ms) | Extra (ms) |
| ------------------------------------------ | ----------: | -------------: | ---------: |
| Initial Run read and authority inheritance |        4.39 |           5.59 |      +1.20 |
| Run visibility                             |       26.29 |          29.31 |      +3.01 |
| Use Recheck                                |      149.37 |         133.38 |     -15.99 |
| Endpoint settings                          |        1.84 |           2.73 |      +0.89 |
| Area initialization check                  |        0.47 |           0.34 |      -0.13 |
| Tool admission and Area load               |        3.63 |           5.22 |      +1.59 |
| Publication                                |        3.56 |          17.59 |     +14.03 |
| Previous record_revision                   |        0.56 |           2.20 |      +1.64 |
| New record_revision                        |        1.26 |          12.06 |     +10.79 |
| Other request work                         |       67.89 |          55.66 |     -12.23 |
| Total                                      |      243.22 |         245.88 |      +2.66 |

The total increased by 1.09%. The budget was 24.32 ms.

End-to-end samples in order (ms)

- 1 file: `[241.049, 243.225, 264.491, 293.464, 242.997]`
- 256 files: `[241.791, 255.066, 244.335, 245.884, 276.198]`

```text
Before tests  1:52  up  5:11, 1 user, load averages: 7.56 8.27 8.21
After tests   1:52  up  5:11, 1 user, load averages: 5.66 7.69 8.00
```

## Proposed decision

Status: proposed, pending the owner's acceptance on
[#940](https://github.com/kent8192/aidash/issues/940). Until then the budget
exception is not accepted.

Accept the measured cost for the current Working Area path. In both runs the
largest repeatable increase among measured stages was publication, where the
new immutable manifest and its retained file references account for roughly
10–11 ms of the increase. #1023 removes Working Area manifests and their
publication path. The observed increases in settings, visibility, and
admission ranged from 0.89 to 3.01 ms. The Use Recheck median gap swung from
+19.73 ms to −15.99 ms between runs. #941's optimization is under review in
[PR #1104](https://github.com/kent8192/aidash/pull/1104).

Both focused cases passed in each run, including the reference-row counts for
every published revision. The default release test build initially failed
because it excludes the fixture DB helpers. After enabling them locally, the
first execution needed a rebuild of the disposable PostgreSQL image because
the local tag was absent at test start. The two measured runs used the same
optimized binary after the image was restored. Full CI, live shell execution,
and #719's Run-start and search checks were outside this measurement.
