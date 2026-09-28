use crate::grapheme::EdgeSkip;
use crate::structs::{FxHashMap, NodeLimits, Similarity};
use crate::{
    Edge, FuzzyAhoCorasick, FuzzyLimits, FuzzyPenalties, FuzzyReplacer, MappingTransition, Node,
    Pattern,
};
use std::collections::VecDeque;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;

/// Builder for [`FuzzyAhoCorasick`].
///
/// ```rust
/// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, SearchOptions};
///
/// let engine = FuzzyAhoCorasickBuilder::new()
///     .case_insensitive(true)
///     .build(["hello", "world"]);
///
/// let result = engine.segment_text("justheLLowOrLd!", &SearchOptions::new().threshold(1.)).unwrap();
/// assert_eq!(result, "just heLLo wOrLd!");
/// ```
#[derive(Debug, Default)]
pub struct FuzzyAhoCorasickBuilder {
    similarity: Option<&'static Similarity>,
    limits: Option<FuzzyLimits>,
    penalties: FuzzyPenalties,
    case_insensitive: bool,
    beam_width: Option<usize>,
    auto_beam: Option<(usize, usize)>,
    /// Multi-character mapping rules `(seq_a, seq_b, score)`, applied bidirectionally.
    mappings: Vec<(String, String, f32)>,
    min_symbol_similarity: f32,
}

impl FuzzyAhoCorasickBuilder {
    /// Start with sensible defaults (borrowed similarity map, 2 edits, etc.)
    #[must_use]
    pub fn new() -> Self {
        Self {
            similarity: None,
            limits: None,
            penalties: FuzzyPenalties::default(),
            case_insensitive: false,
            beam_width: None,
            auto_beam: None,
            mappings: Vec::new(),
            min_symbol_similarity: 0.0,
        }
    }

    /// Provide custom similarity data.
    #[must_use]
    pub fn similarity(mut self, similarity: &'static Similarity) -> Self {
        self.similarity = Some(similarity);
        self
    }

    /// Maximum edit operations (ins/del/sub) allowed while searching.
    #[must_use]
    pub fn fuzzy(mut self, limits: FuzzyLimits) -> Self {
        self.limits = Some(limits.finalize());
        self
    }

    /// Set custom penalty weights (see `FuzzyPenalties`)
    #[must_use]
    pub fn penalties(mut self, penalties: FuzzyPenalties) -> Self {
        self.penalties = penalties;
        self
    }

    /// Enable Unicode‑aware *case‑insensitive* matching.
    #[must_use]
    pub fn case_insensitive(mut self, value: bool) -> Self {
        self.case_insensitive = value;
        self
    }

    /// Set beam width for search. Limits the number of active states to the
    /// top-K candidates with lowest penalties. This trades some accuracy for
    /// significant speed improvements when using high edit limits.
    ///
    /// Recommended values:
    /// - `None` (default): unlimited, explores all states (most accurate)
    /// - `Some(100-500)`: good balance for most use cases
    /// - `Some(50-100)`: faster but may miss some fuzzy matches
    #[must_use]
    pub fn beam_width(mut self, width: usize) -> Self {
        self.beam_width = Some(width);
        self
    }

    /// Enable an *automatic* beam that only engages on pathological inputs. The search runs the
    /// exact unlimited exploration until the number of states it has expanded (across all start
    /// positions) exceeds `budget`; from that point on it beams the frontier to `width` lowest-
    /// penalty candidates for the rest of the search.
    ///
    /// This bounds the worst case (very high edit limits combined with a low similarity threshold,
    /// where the state space explodes but yields few if any extra matches) while leaving ordinary
    /// searches — which never approach `budget` — exact and unaffected. An explicit
    /// [`beam_width`](Self::beam_width) always takes precedence over this.
    #[must_use]
    pub fn auto_beam(mut self, budget: usize, width: usize) -> Self {
        self.auto_beam = Some((budget, width));
        self
    }

    /// Register a multi-character equivalence between two grapheme sequences (e.g. `"æ"` ↔ `"ae"`,
    /// `"ß"` ↔ `"ss"`, `"ks"` ↔ `"x"`). During search either side may stand in for the other; the
    /// substitution is exact (score `1.0`, no penalty) but still counts as one substitution against
    /// the edit limits, exactly like a single-character similarity substitution.
    ///
    /// Mappings are applied bidirectionally. Use [`mapping_scored`](Self::mapping_scored) for a
    /// near-equivalence that should carry a penalty.
    #[must_use]
    pub fn mapping(self, a: impl Into<String>, b: impl Into<String>) -> Self {
        self.mapping_scored(a, b, 1.0)
    }

    /// Like [`mapping`](Self::mapping) but with a similarity `score` in `0.0..=1.0`. The applied
    /// penalty is `substitution * (1 - score)`, so `1.0` is a free exact equivalence and lower
    /// scores make the mapping progressively more expensive.
    #[must_use]
    pub fn mapping_scored(
        mut self,
        a: impl Into<String>,
        b: impl Into<String>,
        score: f32,
    ) -> Self {
        self.mappings.push((a.into(), b.into(), score));
        self
    }

    /// Require every character-level substitution to have at least this similarity (`0.0..=1.0`;
    /// default `0.0`, i.e. no floor). A substitution whose similarity falls below the floor is
    /// rejected outright — a "weakest link" bound (see the Horák et al. paper) that prevents a
    /// single wildly-dissimilar character from being masked by an otherwise-good long match. Exact
    /// matches and explicit [`mapping`](Self::mapping)s (which carry their own scores) are unaffected.
    #[must_use]
    pub fn min_symbol_similarity(mut self, min: f32) -> Self {
        self.min_symbol_similarity = min;
        self
    }

    /// Prefix‑membership‑function – the deeper we are inside a pattern, the
    /// lower the weight (ensures that complete matches rank higher than
    /// partial prefix matches).
    fn pmf(weight: f32, word_len: usize, prefix_len: usize) -> f32 {
        weight * ((word_len - prefix_len + 1) as f32 / word_len as f32)
    }

    /// Build a [`FuzzyReplacer`] from `(pattern, replacement)` pairs: each pattern is matched
    /// fuzzily (with this builder's configuration) and substituted with its paired replacement.
    /// A turnkey alternative to [`build`](Self::build) + [`FuzzyAhoCorasick::replace`].
    #[must_use]
    pub fn build_replacer<T, R>(self, pairs: impl IntoIterator<Item = (T, R)>) -> FuzzyReplacer
    where
        T: Into<Pattern>,
        R: Into<String>,
    {
        let (patterns, replacements): (Vec<_>, Vec<_>) =
            pairs.into_iter().map(|(p, r)| (p.into(), r.into())).unzip();

        FuzzyReplacer {
            engine: self.build(patterns),
            replacements,
        }
    }

    /// Builds an immutable [`FuzzyAhoCorasick`] engine from pattern list.
    ///
    /// ```rust
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, SearchOptions};
    ///
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .case_insensitive(true)
    ///     .build([("Γειά", 1.0), ("σου", 1.0)]);
    ///
    /// assert!(!engine.search("γειά ΣΟΥ!", &SearchOptions::new().threshold(0.8).sorted()).unwrap().is_empty());
    /// ```
    pub fn build<T>(self, inputs: impl IntoIterator<Item = T>) -> FuzzyAhoCorasick
    where
        T: Into<Pattern>,
    {
        let patterns: Vec<Pattern> = inputs.into_iter().map(Into::into).collect();
        let similarity: &'static Similarity = self.similarity.unwrap_or(&DEFAULT_SIMILARITY);

        let mut nodes = vec![Node::new(
            #[cfg(debug_assertions)]
            0,
            #[cfg(debug_assertions)]
            None,
        )];

        // Per-node edit budgets, needed only when some pattern carries its own limits: the search
        // gates a walk on the union of the budgets of every pattern that could still be completed
        // from the node it is sitting at. See `NodeLimits` for why a single pattern's limits are not
        // enough. Both vectors grow in lockstep with `nodes`, and stay empty otherwise.
        let has_pattern_limits = patterns.iter().any(|p| p.limits.is_some());
        let mut node_limits: Vec<NodeLimits> = Vec::new();
        // Nodes on a path of some pattern that relies on the engine-wide limits rather than its own.
        let mut needs_global_limits: Vec<bool> = Vec::new();
        if has_pattern_limits {
            node_limits = vec![NodeLimits::default(); nodes.len()];
            needs_global_limits = vec![false; nodes.len()];
        }

        for (i, pattern) in patterns.iter().enumerate() {
            let mut current: usize = 0;
            // The root is on every pattern's path, so it needs the same treatment as the nodes
            // below -- the walk starts there, and with a zeroed budget every edit from the root
            // would be refused.
            if has_pattern_limits {
                if let Some(lim) = &pattern.limits {
                    node_limits[0].union_with(lim.effective_bounds());
                } else {
                    needs_global_limits[0] = true;
                }
            }
            let word_iter: Vec<String> = if self.case_insensitive {
                UnicodeSegmentation::graphemes(pattern.pattern.as_str(), true)
                    .map(str::to_lowercase)
                    .collect()
            } else {
                UnicodeSegmentation::graphemes(pattern.pattern.as_str(), true)
                    .map(str::to_string)
                    .collect()
            };

            for (j, grapheme) in word_iter.iter().enumerate() {
                let next = if let Some(&next_index) = nodes[current].transitions.get(grapheme) {
                    next_index as usize
                } else {
                    let new_index = nodes.len();
                    nodes[current]
                        .transitions
                        .insert(grapheme.clone(), new_index as u32);
                    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
                    let parent = current as u32;
                    nodes.push(Node::new(
                        #[cfg(debug_assertions)]
                        parent,
                        #[cfg(debug_assertions)]
                        Some(grapheme),
                    ));
                    if has_pattern_limits {
                        node_limits.push(NodeLimits::default());
                        needs_global_limits.push(false);
                    }
                    new_index
                };

                // Track the first pattern to touch this node
                nodes[next].pattern_index.get_or_insert(i);

                // Every pattern that passes through a node can be completed from it, so the node's
                // gate has to cover all of them, not just the one ending there. Merged here while
                // the path is in hand. A pattern that carries no limits of its own falls back to
                // the engine-wide ones, which are not settled yet -- those nodes are noted and
                // folded in once `effective_limits` exists.
                if has_pattern_limits {
                    if let Some(lim) = &pattern.limits {
                        node_limits[next].union_with(lim.effective_bounds());
                    } else {
                        needs_global_limits[next] = true;
                    }
                }

                current = next;

                let updated_weight = Self::pmf(pattern.weight, word_iter.len(), j + 1);
                nodes[current].weight = nodes[current].weight.max(updated_weight);
            }

            nodes[current].output.push(i as u32);
            nodes[current].weight = nodes[current].weight.max(pattern.weight);
        }

        // build failure links...
        // Whether any node ends with a pattern of its own after at least one failure step, i.e.
        // whether some pattern is a proper suffix of another. Tracked here because the walk below
        // already visits nodes in increasing depth, and a failure target is always shallower than
        // the node pointing at it -- so `inherits[fallback]` is settled by the time it is needed.
        let mut inherits = vec![false; nodes.len()];
        let mut has_suffix_patterns = false;

        let mut queue: VecDeque<u32> = VecDeque::new();
        let root_children: Vec<u32> = nodes[0].transitions.values().copied().collect();
        for child in root_children {
            nodes[child as usize].fail = 0;
            queue.push_back(child);
        }

        while let Some(current_u32) = queue.pop_front() {
            let current = current_u32 as usize;
            let transitions: Vec<(String, u32)> = nodes[current]
                .transitions
                .iter()
                .map(|(g, &n)| (g.clone(), n))
                .collect();

            for (g, next) in transitions {
                let mut fail = nodes[current].fail;
                while fail != 0 && !nodes[fail as usize].transitions.contains_key(&g) {
                    fail = nodes[fail as usize].fail;
                }

                let fallback = *nodes[fail as usize].transitions.get(&g).unwrap_or(&0);
                nodes[next as usize].fail = fallback;

                // NB: a node's `output` deliberately holds only the patterns that end *at* that
                // node — not the ones it inherits along this failure link. The fuzzy search reports
                // `output` at the span its walk consumed, which is only correct for a pattern whose
                // own graphemes end there; an inherited one is a suffix of what the walk consumed
                // and would be reported at the wrong span, with the walk's penalties rather than its
                // own alignment. (It is also never the only way to find that match: the search
                // restarts at every start position, so the pattern's own window finds it.)
                //
                // The exact single-pass scan still needs the inherited set, and gets it by walking
                // the failure chain from the reached state — the classic output-link traversal.
                // Doing it there instead of here also drops a build-time quadratic `contains` per
                // inherited entry.

                let inherits_here =
                    !nodes[fallback as usize].output.is_empty() || inherits[fallback as usize];
                inherits[next as usize] = inherits_here;
                has_suffix_patterns |= inherits_here;

                if nodes[next as usize].weight < nodes[fallback as usize].weight {
                    nodes[next as usize].weight = nodes[fallback as usize].weight;
                }

                queue.push_back(next);
            }
        }

        // propagate weights up the fail chain (Horák)
        for i in (1..nodes.len()).rev() {
            let f = nodes[i].fail as usize;
            if nodes[f].weight > nodes[i].weight {
                nodes[i].weight = nodes[f].weight;
            }
        }

        // The engine-wide limits are exactly what the caller set. Nothing is derived from the
        // patterns here: the reason this once was, to stop a walk being blocked at a node no pattern
        // ends, is now handled properly by the per-node union above, which covers every pattern a
        // walk at that node could still complete.
        //
        // Deriving a set was actively wrong, because it became the authoritative fallback for a
        // pattern carrying no limits of its own. The derived set is not `finalize`d, so its unset
        // fields read as *unconstrained* rather than `0` -- mixing a limited pattern with a plain
        // one made the plain one match with unbounded edits. `FuzzyLimits::default` is documented as
        // "no fuzziness", so a pattern that sets no limits has to stay exact.
        let effective_limits = self.limits;

        // Fold the engine-wide limits into every node that a limit-less pattern can be completed
        // from, so those nodes gate on the fallback its own patterns resolve to. With no
        // engine-wide limits either, such a pattern is exact, so those nodes keep a zero budget and
        // no fuzzy walk is started from them.
        if has_pattern_limits {
            if let Some(lim) = effective_limits.as_ref() {
                let global = lim.effective_bounds();
                for (n, &needs) in needs_global_limits.iter().enumerate() {
                    if needs {
                        node_limits[n].union_with(global);
                    }
                }
            }
        }

        // Materialise the flat edge list the search hot path iterates over, now that the trie
        // (including any minimisation) is final. Order follows `transitions`' iteration order —
        // deterministic given the fixed-seed hasher — which is exactly the order the search
        // previously iterated the map in, so tie-breaking among equal-similarity matches is
        // unchanged.
        for node in &mut nodes {
            node.edges = node
                .transitions
                .iter()
                .map(|(g, &next)| Edge::new(g.chars().next().unwrap_or('\0'), next, g.len() == 1))
                .collect();
            // Precompute the ASCII edge-char bitmap the dead-end filter probes (see `Node::edge_bits`).
            // Only single-byte graphemes can have a first `char` < 128, so the bitmap answers every
            // ASCII probe exactly.
            let mut bits = 0u128;
            for edge in &node.edges {
                if edge.is_single_byte() {
                    let idx = edge.first_char as u32;
                    if idx < 128 {
                        bits |= 1u128 << idx;
                    }
                }
            }
            node.edge_bits = bits;
        }

        // Per-node reachable bounds (longest pattern / heaviest weight reachable from each node).
        // Seed each node from the patterns that complete at it, then propagate descendants' values
        // up the transition edges to a fixpoint. The `max` update is monotone and bounded, so this
        // converges even if minimisation turned the trie into a DAG with shared subtrees.
        let mut reach_len: Vec<usize> = vec![0; nodes.len()];
        let mut reach_weight: Vec<f32> = vec![0.0; nodes.len()];
        for (i, node) in nodes.iter().enumerate() {
            for &p in &node.output {
                reach_len[i] = reach_len[i].max(patterns[p as usize].grapheme_len);
                reach_weight[i] = reach_weight[i].max(patterns[p as usize].weight);
            }
        }
        // Iterate high index → low: in the freshly built trie a child always has a higher index
        // than its parent, so descendants are finalised before their parent and a single pass
        // suffices. The `changed` loop only does extra work if minimisation turned the trie into a
        // DAG; `max` is monotone and bounded, so it still converges.
        let mut changed = true;
        while changed {
            changed = false;
            for i in (0..nodes.len()).rev() {
                let (mut best_len, mut best_weight) = (reach_len[i], reach_weight[i]);
                for &child in nodes[i].transitions.values() {
                    best_len = best_len.max(reach_len[child as usize]);
                    best_weight = best_weight.max(reach_weight[child as usize]);
                }
                // `max` is monotone, so a change can only be an increase.
                if best_len > reach_len[i] || best_weight > reach_weight[i] {
                    reach_len[i] = best_len;
                    reach_weight[i] = best_weight;
                    changed = true;
                }
            }
        }
        for (i, node) in nodes.iter_mut().enumerate() {
            let len = reach_len[i] as f32;
            node.prune_len = len;
            node.prune_len_over_weight = len / reach_weight[i];
        }

        // Precompute multi-character mapping transitions, keyed by the node they apply from. Each
        // configured rule becomes two directed rules (bidirectional); for every node we walk the
        // rule's pattern-side grapheme sequence through the trie and, when it forms a valid path,
        // record a transition that consumes the haystack-side sequence and jumps to the node the walk
        // reached. Both sides are grapheme-split and case-folded exactly like patterns, so they line
        // up with the trie edges and the (also folded) haystack graphemes at search time. Only nodes
        // with at least one applicable mapping get an entry.
        let mut mappings: FxHashMap<u32, Box<[MappingTransition]>> = FxHashMap::default();
        if !self.mappings.is_empty() {
            let fold = |s: &str| -> Vec<String> {
                UnicodeSegmentation::graphemes(s, true)
                    .map(|g| {
                        if self.case_insensitive {
                            g.to_lowercase()
                        } else {
                            g.to_string()
                        }
                    })
                    .collect()
            };
            let mut directed: Vec<(Vec<String>, Vec<String>, f32)> = Vec::new();
            for (a, b, score) in &self.mappings {
                let (ga, gb) = (fold(a), fold(b));
                if ga.is_empty() || gb.is_empty() || ga == gb {
                    continue;
                }
                let penalty = self.penalties.substitution * (1.0 - score);
                directed.push((ga.clone(), gb.clone(), penalty));
                directed.push((gb, ga, penalty));
            }
            for start in 0..nodes.len() {
                let mut mts: Vec<MappingTransition> = Vec::new();
                for (pat, hay, penalty) in &directed {
                    let mut cur: usize = start;
                    let mut ok = true;
                    for g in pat {
                        if let Some(&nx) = nodes[cur].transitions.get(g) {
                            cur = nx as usize;
                        } else {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        mts.push(MappingTransition {
                            haystack: hay
                                .iter()
                                .map(|g| g.as_str().into())
                                .collect::<Vec<Box<str>>>()
                                .into_boxed_slice(),
                            next: cur as u32,
                            penalty: *penalty,
                        });
                    }
                }
                if !mts.is_empty() {
                    mappings.insert(start as u32, mts.into_boxed_slice());
                }
            }
        }

        let has_pattern_limits = patterns.iter().any(|p| p.limits.is_some());

        // Fast-path edit ceiling: when the global limits only constrain total `edits` (all
        // per-type fields `None`) and no pattern has its own limits, the hot loop can check
        // `edits <= max_edits_fast` without loading `self.limits` or branching on five
        // `Option<u8>` fields. `255` disables the fast path (complex limits or per-pattern
        // limits); `0` means exact-only (no limits set at all).
        let max_edits_fast = if has_pattern_limits {
            255
        } else {
            match &effective_limits {
                None => 0, // exact match only
                Some(lim) => match lim.edits {
                    Some(e)
                        if lim.insertions.is_none()
                            && lim.deletions.is_none()
                            && lim.substitutions.is_none()
                            && lim.swaps.is_none() =>
                    {
                        e
                    }
                    _ => 255,
                },
            }
        };

        let edge_skip = EdgeSkip::new(nodes[0].edge_bits);
        FuzzyAhoCorasick {
            nodes,
            patterns,
            similarity,
            limits: effective_limits,
            penalties: self.penalties,
            case_insensitive: self.case_insensitive,
            edge_skip,
            has_suffix_patterns,
            has_pattern_limits,
            node_limits,
            max_edits_fast,
            mappings,
            beam_width: self.beam_width,
            auto_beam: self.auto_beam,
            min_symbol_similarity: self.min_symbol_similarity,
        }
    }
}

/* -------------------------------------------------------------------------
 *  Default similarity
 * ---------------------------------------------------------------------- */

/// Singleton that stores the lazily‑initialised vowel/consonant similarity data.
static DEFAULT_SIMILARITY: LazyLock<Similarity> = LazyLock::new(|| {
    let mut map = FxHashMap::default();
    let vowels = ['a', 'e', 'i', 'o', 'u'];
    let consonants = (b'a'..=b'z')
        .map(|b| b as char)
        .filter(|c| !vowels.contains(c))
        .collect::<Vec<_>>();

    // Vowel ↔ vowel similarities.
    for &a in &vowels {
        for &b in &vowels {
            if a != b {
                map.insert((a, b), 0.6);
            }
        }
    }
    // Consonant ↔ consonant similarities.
    for &a in &consonants {
        for &b in &consonants {
            if a != b {
                map.insert((a, b), 0.4);
            }
        }
    }
    // Common OCR/typo confusions
    map.insert(('o', '0'), 0.6);
    map.insert(('0', 'o'), 0.6);
    map.insert(('l', '1'), 0.7);
    map.insert(('1', 'l'), 0.7);
    map.insert(('i', '1'), 0.6);
    map.insert(('1', 'i'), 0.6);
    map.insert(('s', '5'), 0.5);
    map.insert(('5', 's'), 0.5);
    Similarity::from_map(map)
});
