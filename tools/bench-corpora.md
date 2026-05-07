# Bench corpora — Sprint 6 RC

The `extract` benches run a synthetic in-memory ZIP by default so CI stays
hermetic. Real-world numbers (per `docs/01-plan/performance.md` §6) come
from running the same benches against an external corpus via the
`OTTERZIP_BENCH_CORPUS` environment variable.

## Recommended corpora

| Corpus | URL | Approx. size | Notes |
|---|---|---|---|
| Silesia | http://sun.aei.polsl.pl/~sdeor/index.php?page=silesia | ~203 MB raw, ~75 MB ZIP | Repackage as a single ZIP to use |
| Canterbury | https://corpus.canterbury.ac.nz/descriptions/ | ~3 MB | Small, sanity-only |
| enwik9 | https://mattmahoney.net/dc/enwik9.zip | 322 MB ZIP | Large-text benchmark |
| Chromium tarball | github.com/chromium/chromium archive | ~5 GB | Real-world mixed |

## Running locally

```pwsh
# Stage Silesia as a single ZIP
$silesia = "C:/corpora/silesia.zip"
$Env:OTTERZIP_BENCH_CORPUS = $silesia

# Run the throughput benchmarks
cargo bench -p otterzip-core --bench extract -- --warm-up-time 2 --measurement-time 10

# Reset
Remove-Item Env:OTTERZIP_BENCH_CORPUS
```

The bench JSON output lives in `target/criterion/`. Compare across
revisions with `cargo install critcmp && critcmp baseline new`.

## CI regression gate (Sprint 6)

`ci-rust.yml`'s `bench-smoke` job runs the synthetic fixture only — fast
enough to gate every PR (~30 s). The "real" Silesia gate is a separate
post-merge job that runs nightly and compares against a stored baseline:

- PR threshold: -5% throughput regression triggers a comment + non-blocking
- Main threshold: -2% throughput regression blocks the deploy pipeline

Until that nightly runner is provisioned, performance regressions are
caught manually on dev machines using the steps above.
