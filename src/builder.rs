use crate::structs::{FxHashMap, Similarity};
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
/// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder};
///
/// let engine = FuzzyAhoCorasickBuilder::new()
///     .case_insensitive(true)
///     .build(["hello", "world"]);
///
/// let result = engine.segment_text("justheLLowOrLd!", 1.);
/// assert_eq!(result, "just heLLo wOrLd!");
/// ```
#[derive(Debug, Default)]
pub struct FuzzyAhoCorasickBuilder {
    minimize_lambda: Option<f32>,
    similarity: Option<&'static Similarity>,
    limits: Option<FuzzyLimits>,
    penalties: FuzzyPenalties,
    case_insensitive: bool,
    beam_width: Option<usize>,
    auto_beam: Option<(usize, usize)>,
    /// Multi-character mapping rules `(seq_a, seq_b, score)`, applied bidirectionally.
    mappings: Vec<(String, String, f32)>,
}

impl FuzzyAhoCorasickBuilder {
    /// Start with sensible defaults (borrowed similarity map, 2 edits, etc.)
    #[must_use]
    pub fn new() -> Self {
        Self {
            minimize_lambda: None,
            similarity: None,
            limits: None,
            penalties: FuzzyPenalties::default(),
            case_insensitive: false,
            beam_width: None,
            auto_beam: None,
            mappings: Vec::new(),
        }
    }

    /// Enables λ-minimisation with given tolerance.
    #[must_use]
    pub fn minimize(mut self, lambda: f32) -> Self {
        self.minimize_lambda = Some(lambda);
        self
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

    /// Prefix‑membership‑function – the deeper we are inside a pattern, the
    /// lower the weight (ensures that complete matches rank higher than
    /// partial prefix matches).
    fn pmf(weight: f32, word_len: usize, prefix_len: usize) -> f32 {
        weight * ((word_len - prefix_len + 1) as f32 / word_len as f32)
    }

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
    /// use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;
    ///
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .case_insensitive(true)
    ///     .build([("Γειά", 1.0), ("σου", 1.0)]);
    ///
    /// assert!(!engine.search("γειά ΣΟΥ!", 0.8).is_empty());
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

        for (i, pattern) in patterns.iter().enumerate() {
            let mut current: usize = 0;
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
                    new_index
                };

                // Track the first pattern to touch this node
                nodes[next].pattern_index.get_or_insert(i);

                current = next;

                let updated_weight = Self::pmf(pattern.weight, word_iter.len(), j + 1);
                nodes[current].weight = nodes[current].weight.max(updated_weight);
            }

            nodes[current].output.push(i as u32);
            nodes[current].weight = nodes[current].weight.max(pattern.weight);
        }

        // build failure links...
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

                for &entry in &nodes[fallback as usize].output.clone() {
                    if !nodes[next as usize].output.contains(&entry) {
                        nodes[next as usize].output.push(entry);
                    }
                }

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

        if let Some(lambda) = self.minimize_lambda {
            let mut classes: Vec<usize> = (0..nodes.len()).collect();
            let mut reprs: Vec<Node> = Vec::new();

            for (i, node) in nodes.iter().enumerate() {
                if let Some((j, _)) = reprs.iter().enumerate().find(|(_, rep)| {
                    (rep.weight - node.weight).abs() <= lambda
                        && rep.output == node.output
                        && rep.transitions == node.transitions
                        && rep.fail == node.fail
                        && rep.epsilon == node.epsilon
                }) {
                    classes[i] = j;
                } else {
                    classes[i] = reprs.len();
                    reprs.push(node.clone());
                }
            }

            // remap all internal links
            for rep in &mut reprs {
                if let Some(e) = rep.epsilon {
                    rep.epsilon = Some(classes[e as usize] as u32);
                }
                rep.fail = classes[rep.fail as usize] as u32;
                rep.transitions = rep
                    .transitions
                    .iter()
                    .map(|(k, &v)| (k.clone(), classes[v as usize] as u32))
                    .collect();
            }

            nodes = reprs;
        }

        // Compute effective limits: if no global limits are set but patterns have limits,
        // derive a permissive global limit from the max of all pattern limits.
        // This fixes the bug where deletions at non-final nodes were blocked.
        let effective_limits = self.limits.or_else(|| {
            let mut max_edits = None;
            let mut max_insertions = None;
            let mut max_deletions = None;
            let mut max_substitutions = None;
            let mut max_swaps = None;
            let mut any_pattern_has_limits = false;

            for p in &patterns {
                if let Some(ref lim) = p.limits {
                    any_pattern_has_limits = true;
                    if let Some(e) = lim.edits {
                        max_edits = Some(max_edits.unwrap_or(0).max(e));
                    }
                    if let Some(i) = lim.insertions {
                        max_insertions = Some(max_insertions.unwrap_or(0).max(i));
                    }
                    if let Some(d) = lim.deletions {
                        max_deletions = Some(max_deletions.unwrap_or(0).max(d));
                    }
                    if let Some(s) = lim.substitutions {
                        max_substitutions = Some(max_substitutions.unwrap_or(0).max(s));
                    }
                    if let Some(sw) = lim.swaps {
                        max_swaps = Some(max_swaps.unwrap_or(0).max(sw));
                    }
                }
            }

            if any_pattern_has_limits {
                Some(FuzzyLimits {
                    edits: max_edits,
                    insertions: max_insertions,
                    deletions: max_deletions,
                    substitutions: max_substitutions,
                    swaps: max_swaps,
                })
            } else {
                None
            }
        });

        // Materialise the flat edge list the search hot path iterates over, now that the trie
        // (including any minimisation) is final. Order follows `transitions`' iteration order —
        // deterministic given the fixed-seed hasher — which is exactly the order the search
        // previously iterated the map in, so tie-breaking among equal-similarity matches is
        // unchanged.
        for node in &mut nodes {
            node.edges = node
                .transitions
                .iter()
                .map(|(g, &next)| Edge {
                    first_char: g.chars().next().unwrap_or('\0'),
                    next,
                })
                .collect();
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

        FuzzyAhoCorasick {
            nodes,
            patterns,
            similarity,
            limits: effective_limits,
            penalties: self.penalties,
            case_insensitive: self.case_insensitive,
            has_pattern_limits,
            mappings,
            beam_width: self.beam_width,
            auto_beam: self.auto_beam,
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
