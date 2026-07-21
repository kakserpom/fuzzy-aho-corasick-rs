# Scoring & Thresholds

Every candidate match earns a **similarity score**, and only candidates scoring at or above the
threshold you pass to a search are returned. The score also decides how matches rank against each
other.

## The formula

For a candidate matched against a pattern of `N` grapheme clusters, having accumulated total edit
`penalties`:

```text
similarity = (N - penalties) / N * weight
```

- **`N`** is the pattern length in graphemes. Longer patterns dilute a fixed penalty, so one typo in
  a long word costs proportionally less than one in a short word.
- **`penalties`** is the sum of the per-edit costs (see [Penalties](../building/penalties.md)). A
  substitution's cost is scaled by symbol similarity, so a near-miss like `o`↔`0` costs less than a
  wholly unrelated substitution.
- **`weight`** is the pattern's weight (default `1.0`). See [Patterns & Weights](../building/patterns.md).

With the default weight of `1.0`, a perfect match scores `1.0` and the score falls toward `0.0` as
penalties accumulate. Weights above `1.0` can push an important pattern's score above `1.0` to
prioritize it.

## The threshold

Every search takes a `threshold` in `0.0..=1.0`:

```rust
# use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
# let engine = FuzzyAhoCorasickBuilder::new().fuzzy(FuzzyLimits::new().edits(2)).build(["needle"]);
let strict  = engine.search("neeedle", 0.9);  // fewer, higher-quality matches
let lenient = engine.search("neeedle", 0.6);  // more matches, more noise
```

The threshold is your primary quality knob. It is also a performance knob: a higher threshold lets
the engine prune weak partial matches earlier, so it does less work.

## Worked example

Take the pattern `hello` (`N = 5`) and the text `helllo` (one extra `l`, i.e. one insertion). With
the default insertion penalty (~0.52):

```text
similarity = (5 - 0.52) / 5 ≈ 0.90
```

So `helllo` matches `hello` at ~0.90 — comfortably above a 0.8 threshold, but it would be rejected at
0.95.

## Additive scoring and its trade-off

The score is **additive**: each edit contributes independently. This is robust and predictable, but
it means a single very-bad substitution can be hidden inside an otherwise-excellent long match — one
`sim = 0` substitution in a 20-grapheme pattern still scores ~0.93. If you need to forbid that, the
[weakest-link floor](../similarity/floor.md) rejects any substitution below a per-symbol similarity,
independent of the overall score.

## Limits vs. threshold

There are two independent gates a match must pass:

1. **[Edit limits](../building/builder.md)** — a hard cap on the *number* of each edit type (and the
   total). A candidate exceeding any limit is discarded outright.
2. **Threshold** — a floor on the *score*.

Limits bound the search space (and worst-case cost); the threshold selects quality within it. You
usually set both: limits to say "no more than 2 edits", and a threshold to say "and it must still
look at least 80% like the pattern".
