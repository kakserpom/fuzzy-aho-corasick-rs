# Acknowledgements

The fuzzy automaton is based on the research paper
[**Fuzzified Aho–Corasick Search Automata**](https://github.com/kakserpom/fuzzy-aho-corasick-rs/blob/master/DOCS/ias10_horak.pdf)
by Zdeněk Horák, Václav Snášel, Ajith Abraham, and Aboul Ella Hassanien (IAS 2010).

The crate adapts the paper's core idea — a fuzzified Aho–Corasick automaton with a similarity-aware
transition model — into an additive, length-normalized scoring scheme with per-edit-type penalties
and limits, and extends it with transpositions, multi-character mappings, the weakest-link floor,
streaming, and a bit-parallel pre-filter. The [How It Works](internals.md) chapter describes the
resulting engine.

## Project links

- **Repository:** <https://github.com/kakserpom/fuzzy-aho-corasick-rs>
- **crates.io:** <https://crates.io/crates/fuzzy-aho-corasick>
- **API docs:** <https://docs.rs/fuzzy-aho-corasick>

## License

The crate is distributed under the **MIT License**. See the
[`LICENSE`](https://github.com/kakserpom/fuzzy-aho-corasick-rs/blob/master/LICENSE) file for details.

## Contributing

Issues and pull requests are welcome on GitHub. The test suite (`cargo test`), Clippy
(`cargo clippy --all-targets -- -D warnings`), and formatting (`cargo fmt --all -- --check`) all run
in CI; running them locally before opening a PR keeps the loop fast.
