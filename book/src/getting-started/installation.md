# Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
fuzzy-aho-corasick = "0.4"
```

Or with `cargo add`:

```sh
cargo add fuzzy-aho-corasick
```

The crate has a single runtime dependency ([`unicode-segmentation`](https://crates.io/crates/unicode-segmentation))
and builds on stable Rust (edition 2024).

Then bring the common types into scope:

```rust
use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
```

The most useful items are re-exported at the crate root:

| Item | Role |
| --- | --- |
| [`FuzzyAhoCorasickBuilder`] | Configure and build an engine. |
| [`FuzzyAhoCorasick`] | The immutable engine you query. |
| [`FuzzyLimits`] | Edit-count limits (global or per-pattern). |
| [`FuzzyPenalties`] | Per-edit-type cost tuning. |
| [`Pattern`] | A pattern with optional weight / limits / id. |
| [`FuzzyMatch`] | A single match result. |
| [`FuzzyReplacer`] | Turnkey find-and-replace. |

Everything else (the similarity table type, streaming match type, and so on) lives under the same
crate root or the `structs` module.

[`FuzzyAhoCorasickBuilder`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasickBuilder.html
[`FuzzyAhoCorasick`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyAhoCorasick.html
[`FuzzyLimits`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyLimits.html
[`FuzzyPenalties`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyPenalties.html
[`Pattern`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.Pattern.html
[`FuzzyMatch`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/structs/struct.FuzzyMatch.html
[`FuzzyReplacer`]: https://docs.rs/fuzzy-aho-corasick/latest/fuzzy_aho_corasick/struct.FuzzyReplacer.html
