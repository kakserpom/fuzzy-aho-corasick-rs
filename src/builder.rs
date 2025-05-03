use crate::{FuzzyAhoCorasick, FuzzyLimits, FuzzyPenalties, FuzzyReplacer, Node, Pattern};
use std::collections::{BTreeMap, VecDeque};
use unicode_segmentation::UnicodeSegmentation;

/// Builder for [`FuzzyAhoCorasick`].
///
/// ```rust
/// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder};
///
/// let engine = FuzzyAhoCorasickBuilder::new()
///     .case_insensitive(true)
///     .non_overlapping(true)
///     .build(["hello", "world"]);
///
/// let result = engine.segment_text("HeLLo WoRLd!", 1.);
/// assert_eq!(result, "HeLLo WoRLd !");
/// ```
#[derive(Debug, Default)]
pub struct FuzzyAhoCorasickBuilder {
    minimize_lambda: Option<f32>,
    similarity: Option<&'static BTreeMap<(char, char), f32>>,
    limits: Option<FuzzyLimits>,
    penalties: FuzzyPenalties,
    non_overlapping: bool,
    case_insensitive: bool,
}

impl FuzzyAhoCorasickBuilder {
    /// Start with sensible defaults (borrowed similarity map, 2 edits, etc.)
    pub fn new() -> Self {
        Self {
            minimize_lambda: None,
            similarity: None,
            limits: None,
            penalties: FuzzyPenalties::default(),
            non_overlapping: false,
            case_insensitive: false,
        }
    }

    /// Enables λ-minimisation with given tolerance.
    pub fn minimize(mut self, lambda: f32) -> Self {
        self.minimize_lambda = Some(lambda);
        self
    }

    /// Provide a custom similarity map.
    pub fn similarity(mut self, map: &'static BTreeMap<(char, char), f32>) -> Self {
        self.similarity = Some(map);
        self
    }

    /// Maximum edit operations (ins/del/sub) allowed while searching.
    pub fn fuzzy(mut self, limits: FuzzyLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    pub fn penalties(mut self, penalties: FuzzyPenalties) -> Self {
        self.penalties = penalties;
        self
    }

    /// Disallow overlapping matches (the engine will greedily choose the
    /// longest non‑overlapping matches from left to right).
    pub fn non_overlapping(mut self, value: bool) -> Self {
        self.non_overlapping = value;
        self
    }

    /// Enable Unicode‑aware *case‑insensitive* matching.
    pub fn case_insensitive(mut self, value: bool) -> Self {
        self.case_insensitive = value;
        self
    }

    /// Prefix‑membership‑function – the deeper we are inside a pattern, the
    /// lower the weight (ensures that complete matches rank higher than
    /// partial prefix matches).
    fn pmf(weight: f32, word_len: usize, prefix_len: usize) -> f32 {
        weight * ((word_len - prefix_len + 1) as f32 / word_len as f32)
    }

    pub fn build_replacer<'a, T>(
        self,
        pairs: impl IntoIterator<Item = (T, &'a str)>,
    ) -> FuzzyReplacer<'a>
    where
        T: Into<Pattern>,
    {
        let (patterns, replacements): (Vec<_>, Vec<_>) =
            pairs.into_iter().map(|(p, r)| (p.into(), r)).unzip();

        FuzzyReplacer {
            engine: self.non_overlapping(true).build(patterns),
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
        let similarity: &'static BTreeMap<(_, _), _> =
            self.similarity.unwrap_or_else(|| default_similarity_map());

        let mut nodes = vec![Node::new(
            #[cfg(debug_assertions)]
            0,
            #[cfg(debug_assertions)]
            None,
        )];

        for (i, pattern) in patterns.iter().enumerate() {
            let mut current = 0;
            let word_iter: Vec<String> = if self.case_insensitive {
                UnicodeSegmentation::graphemes(pattern.pattern.as_str(), true)
                    .map(|g| g.to_lowercase())
                    .collect()
            } else {
                UnicodeSegmentation::graphemes(pattern.pattern.as_str(), true)
                    .map(|g| g.to_string())
                    .collect()
            };

            for (j, grapheme) in word_iter.iter().enumerate() {
                let next = if let Some(&next_index) = nodes[current].transitions.get(grapheme) {
                    next_index
                } else {
                    let new_index = nodes.len();
                    nodes[current]
                        .transitions
                        .insert(grapheme.clone(), new_index);
                    nodes.push(Node::new(
                        #[cfg(debug_assertions)]
                        current,
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

            nodes[current].output.push(i);
            nodes[current].weight = nodes[current].weight.max(pattern.weight);
        }

        // build failure links...
        let mut queue = VecDeque::new();
        let root_children: Vec<usize> = nodes[0].transitions.values().copied().collect();
        for child in root_children {
            nodes[child].fail = 0;
            queue.push_back(child);
        }

        while let Some(current) = queue.pop_front() {
            let transitions: Vec<(String, usize)> = nodes[current]
                .transitions
                .iter()
                .map(|(g, &n)| (g.clone(), n))
                .collect();

            for (g, next) in transitions {
                let mut fail = nodes[current].fail;
                while fail != 0 && !nodes[fail].transitions.contains_key(&g) {
                    fail = nodes[fail].fail;
                }

                let fallback = *nodes[fail].transitions.get(&g).unwrap_or(&0);
                nodes[next].fail = fallback;

                for &entry in &nodes[fallback].output.clone() {
                    if !nodes[next].output.contains(&entry) {
                        nodes[next].output.push(entry);
                    }
                }

                if nodes[next].weight < nodes[fallback].weight {
                    nodes[next].weight = nodes[fallback].weight;
                }

                queue.push_back(next);
            }
        }

        // propagate weights up the fail chain (Horák)
        for i in (1..nodes.len()).rev() {
            let f = nodes[i].fail;
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
                    rep.epsilon = Some(classes[e]);
                }
                rep.fail = classes[rep.fail];
                rep.transitions = rep
                    .transitions
                    .iter()
                    .map(|(k, &v)| (k.clone(), classes[v]))
                    .collect();
            }

            nodes = reprs;
        }

        FuzzyAhoCorasick {
            nodes,
            patterns,
            similarity,
            limits: self.limits,
            penalties: self.penalties,
            non_overlapping: self.non_overlapping,
            case_insensitive: self.case_insensitive,
        }
    }
}

/* -------------------------------------------------------------------------
 *  Default similarity map
 * ---------------------------------------------------------------------- */

/// Singleton that stores the lazily‑initialised vowel/consonant similarity map.
static DEFAULT_SIMILARITY_MAP: std::sync::OnceLock<BTreeMap<(char, char), f32>> =
    std::sync::OnceLock::new();

/// Returns the default similarity map (vowels ≈ vowels, consonants ≈ consonants).
fn default_similarity_map() -> &'static BTreeMap<(char, char), f32> {
    DEFAULT_SIMILARITY_MAP.get_or_init(|| {
        let mut map = BTreeMap::new();
        let vowels = ['a', 'e', 'i', 'o', 'u'];
        let consonants = (b'a'..=b'z')
            .map(|b| b as char)
            .filter(|c| !vowels.contains(c))
            .collect::<Vec<_>>();

        // Vowel ↔ vowel similarities.
        for &a in &vowels {
            for &b in &vowels {
                map.insert((a, b), if a == b { 1.0 } else { 0.8 });
            }
        }
        // Consonant ↔ consonant similarities.
        for &a in &consonants {
            for &b in &consonants {
                map.insert((a, b), if a == b { 1.0 } else { 0.6 });
            }
        }
        map
    })
}
