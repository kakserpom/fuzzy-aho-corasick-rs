# Tuning & Tips

The engine is built once and cheap to query repeatedly. A few habits keep searches fast and results
clean.

## Filter early

- **Raise the threshold.** A higher [similarity threshold](../concepts/scoring.md) prunes weak
  partial matches before they expand — it improves both quality and speed. It is the single most
  effective knob.
- **Tighten edit limits.** [`FuzzyLimits`](../building/builder.md) per pattern, when you know the
  expected error characteristics, cuts the explored state space directly.

## Shape the cost model to your domain

- Use the [similarity table](../similarity/custom.md) for look-alike single characters (OCR glyphs,
  homoglyphs) so those substitutions are cheap and everything else stays expensive.
- Use [`FuzzyPenalties`](../building/penalties.md) to make whole edit *types* cheaper or pricier.
- Use the [weakest-link floor](../similarity/floor.md) when a single bad character should disqualify
  a match regardless of length.

## Pick the right entry point

- Prefer `search_non_overlapping*` over resolving overlaps yourself.
- Use `search_unsorted` as the primitive when you want to build a custom ranking/selection pipeline.

## Guard against pathological input

Combining a high edit budget with a low threshold is the classic slow case. Add
[`auto_beam`](bounding.md) (keeps common cases exact) or an explicit `beam_width` when limits are high
and thresholds low, especially for untrusted input.

## Reach for the specialized paths when they fit

- **[Streaming](../streaming/search.md)** for large or incremental inputs (constant memory), and its
  **parallel** forms to use all cores on CPU-bound scans.
- **[The pre-filter](prefilter.md)** for large, sparse inputs where most of the text can't match — a
  big speedup with identical results, and a safe fallback when it doesn't apply.

## Size expectations

A single [`search`](../searching/search.md) call keeps grapheme positions as `u32`, so one haystack
should be well under ~4 GiB; beyond that (or for unbounded streams), use the
[streaming API](../streaming/search.md).

## Measuring

Benchmark with your real patterns and representative text — throughput depends heavily on pattern
count/length, edit budget, threshold, and match density. The repository ships
[Criterion benchmarks](https://github.com/kakserpom/fuzzy-aho-corasick-rs) (`cargo bench`) and the
`bitap_prototype` / `replace_bench` examples as starting points.
