use crate::{FuzzyAhoCorasick, FuzzyLimits, FuzzyPenalties, FuzzyReplacer, Node, Pattern};
use itertools::Itertools;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    /// max_word_len, max_error
    minimize: Option<(usize, f32)>,
    similarity: Option<&'static BTreeMap<(char, char), f32>>,
    limits: Option<FuzzyLimits>,
    penalties: FuzzyPenalties,
    case_insensitive: bool,
}

impl FuzzyAhoCorasickBuilder {
    /// Start with sensible defaults (borrowed similarity map, 2 edits, etc.)
    #[must_use]
    pub fn new() -> Self {
        Self {
            minimize: None,
            similarity: None,
            limits: None,
            penalties: FuzzyPenalties::default(),
            case_insensitive: false,
        }
    }

    /// Enables minimization of the automaton with guaranteed error bound.
    ///
    /// For any word `w` of length ≤ `max_word_len`, the difference
    /// in match score will be less than `max_error`:
    ///
    /// |fM(w) - fMλ(w)| < max_error
    #[must_use]
    pub fn minimize_lambda(mut self, max_word_len: usize, max_error: f32) -> Self {
        self.minimize = Some((max_word_len, max_error));
        self
    }
    /// Provide a custom similarity map.
    #[must_use]
    pub fn similarity(mut self, map: &'static BTreeMap<(char, char), f32>) -> Self {
        self.similarity = Some(map);
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

    /// Prefix Membership Function (PMF)
    ///
    /// Computes the membership degree of a trie state based on the length of the matched prefix.
    /// This function is non-increasing with respect to the prefix length.
    ///
    /// # Arguments
    /// * `weight` - The original membership weight of the complete word (e.g., 0.8).
    /// * `word_length` - The total number of graphemes in the original word.
    /// * `prefix_length` - The number of graphemes matched so far in the trie.
    ///
    /// # Returns
    /// The membership degree for the current prefix state.
    ///
    /// # Example
    /// ```
    /// use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;
    /// let weight = 0.8;
    /// let pmf_value = FuzzyAhoCorasickBuilder::pmf(weight, 4, 2); // Halfway through a 4-char word
    /// assert_eq!(pmf_value, 0.4);
    /// ```
    pub fn pmf(weight: f32, word_length: usize, prefix_length: usize) -> f32 {
        // Ensure word_length is not zero to avoid division by zero.
        if word_length == 0 {
            return 0.0;
        }

        // Calculate the ratio of the prefix to the full word.
        let ratio = (prefix_length as f32) / (word_length as f32);

        // Return the weight scaled by the ratio.
        // This creates a linear decay: PMF(weight, n, j) = weight * (j / n)
        weight * ratio
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

    /// Groups nodes into clusters where states are "lambda-similar".
    /// Two nodes are compatible if:
    /// - Their weights (S) differ by less than lambda
    /// - They are both final or both non-final
    /// - Their incoming and outgoing transitions are similar for all symbols
    pub(crate) fn cluster_nodes(nodes: &[Node], lambda: f32) -> Vec<Vec<usize>> {
        // 1. Кластеризация состояний
        let mut clusters: Vec<Vec<usize>> = Vec::new();

        for i in 0..nodes.len() {
            let mut placed = false;
            for cluster in &mut clusters {
                if Self::nodes_compatible(nodes, cluster, i, lambda) {
                    cluster.push(i);
                    placed = true;
                    break;
                }
            }
            if !placed {
                clusters.push(vec![i]);
            }
        }

        clusters
    }

    /// Builds an immutable [`FuzzyAhoCorasick`] engine from pattern list.
    ///
    /// ```rust
    /// use fuzzy_aho_corasick::FuzzyAhoCorasickBuilder;
    ///
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .case_insensitive(true)
    ///     .build([("Γειά", 1.0), ("σου", 1.0)]);
    /// ```
    pub fn build<T>(self, inputs: impl IntoIterator<Item = T>) -> FuzzyAhoCorasick
    where
        T: Into<Pattern>,
    {
        let mut patterns: Vec<Pattern> = inputs.into_iter().map(Into::into).collect();
        let similarity: &'static BTreeMap<(_, _), _> =
            self.similarity.unwrap_or(&DEFAULT_SIMILARITY_MAP);

        // Start with root node (index 0)
        let mut nodes = vec![Node::new(
            #[cfg(debug_assertions)]
            0,
            #[cfg(debug_assertions)]
            None,
        )];

        // Map: node index → pattern index (for reconstruction after minimization)
        let mut node_to_pattern_map = BTreeMap::new();

        // === PHASE 1: Build Fuzzy Trie (Section V-C) ===
        for (i, pattern) in patterns.iter().enumerate() {
            let mut current = 0; // start at root
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
                let next = {
                    let new_index = nodes.len();
                    let transitions = nodes[current]
                        .transitions
                        .entry(grapheme.clone())
                        .or_insert_with(Vec::new);

                    // Check if a transition to a new node already exists
                    if let Some(&target) = transitions.iter().find(|&&target| target == new_index) {
                        target
                    } else {
                        // Create new node and add transition
                        transitions.push(new_index);
                        nodes.push(Node::new(
                            #[cfg(debug_assertions)]
                            current,
                            #[cfg(debug_assertions)]
                            Some(grapheme),
                        ));
                        new_index
                    }
                };

                // Track first pattern to reach this node
                nodes[next].pattern_index.get_or_insert(i);

                current = next;

                // Update node weight using PMF (Section V-C)
                let updated_weight = Self::pmf(pattern.weight, word_iter.len(), j + 1);
                nodes[current].weight = nodes[current].weight.max(updated_weight);
            }

            // Mark end of pattern
            node_to_pattern_map.insert(current, i);
            nodes[current].output.push(i);
            nodes[current].weight = nodes[current].weight.max(pattern.weight);
        }

        // === PHASE 2: Build Failure Links (Section V-D) ===
        let mut queue = VecDeque::new();
        let root_children: Vec<usize> = nodes[0].transitions.values().flatten().copied().collect();

        for child in root_children {
            nodes[child].fail = 0;
            queue.push_back(child);
        }

        while let Some(current) = queue.pop_front() {
            for (g, targets) in &nodes[current].transitions.clone() {
                for &next in targets {
                    let mut fail = nodes[current].fail;
                    while fail != 0 && !nodes[fail].transitions.contains_key(g) {
                        fail = nodes[fail].fail;
                    }

                    let fallback_node = if fail == 0 {
                        0
                    } else {
                        *nodes[fail]
                            .transitions
                            .get(g)
                            .and_then(|targets| targets.first())
                            .unwrap_or(&0)
                    };

                    nodes[next].fail = fallback_node;

                    // Inherit outputs from fail state
                    for &entry in &nodes[fallback_node].output.clone() {
                        if !nodes[next].output.contains(&entry) {
                            nodes[next].output.push(entry);
                        }
                    }

                    // Inherit weight from fail state
                    if nodes[next].weight < nodes[fallback_node].weight {
                        nodes[next].weight = nodes[fallback_node].weight;
                    }

                    queue.push_back(next);
                }
            }
        }

        // Propagate weights up the fail chain (Horák)
        for i in (1..nodes.len()).rev() {
            let f = nodes[i].fail;
            if nodes[f].weight > nodes[i].weight {
                nodes[i].weight = nodes[f].weight;
            }
        }

        // === PHASE 3: Minimize Automaton (Section IV) ===
        if let Some((max_word_len, max_error)) = self.minimize {
            let lambda = max_error / (max_word_len as f32 + 2.0);

            let clusters = Self::cluster_nodes(&nodes, lambda);
            let cluster_map: BTreeMap<usize, usize> = clusters
                .iter()
                .enumerate()
                .flat_map(|(cluster_id, cluster)| {
                    cluster.iter().map(move |&old_idx| (old_idx, cluster_id))
                })
                .collect();

            let num_clusters = clusters.len();

            // Sλ: average weight of each cluster
            let s_lambda: Vec<f32> = clusters
                .iter()
                .map(|cluster| {
                    cluster.iter().map(|&i| nodes[i].weight).sum::<f32>() / cluster.len() as f32
                })
                .collect();

            // Fλ: clusters where ALL nodes are final
            let f_lambda: BTreeSet<usize> = clusters
                .iter()
                .enumerate()
                .filter(|(_, cluster)| cluster.iter().all(|&i| nodes[i].pattern_index.is_some()))
                .map(|(idx, _)| idx)
                .collect();

            // Create new minimized nodes
            let mut new_nodes: Vec<Node> = (0..num_clusters)
                .map(|i| Node {
                    pattern_index: None,
                    transitions: BTreeMap::new(),
                    fail: 0,
                    output: vec![],
                    weight: s_lambda[i],
                    #[cfg(debug_assertions)]
                    parent: 0,
                    #[cfg(debug_assertions)]
                    grapheme: None,
                })
                .collect();

            // Rebuild transitions between clusters
            let all_chars: Vec<&str> = nodes
                .iter()
                .flat_map(|node| node.transitions.keys())
                .map(String::as_str)
                .unique()
                .collect();
            for c in &all_chars {
                for src_cluster_idx in 0..num_clusters {
                    for dst_cluster_idx in 0..num_clusters {
                        let src_cluster = &clusters[src_cluster_idx];
                        let dst_cluster = &clusters[dst_cluster_idx];

                        let mut total_weight = 0.0;
                        let mut count = 0;

                        for &src_old in src_cluster {
                            if let Some(targets) = nodes[src_old].transitions.get(&c.to_string()) {
                                for &dst_old in targets {
                                    if dst_cluster.contains(&dst_old) {
                                        total_weight += nodes[dst_old].weight;
                                        count += 1;
                                    }
                                }
                            }
                        }

                        if count > 0 {
                            let avg_weight =
                                total_weight / (src_cluster.len() * dst_cluster.len()) as f32;
                            if avg_weight > 0.0 {
                                new_nodes[src_cluster_idx]
                                    .transitions
                                    .entry(c.to_string())
                                    .or_insert_with(Vec::new)
                                    .push(dst_cluster_idx);
                            }
                        }
                    }
                }
            }

            // Restore fail links using cluster_map
            for old_idx in 0..nodes.len() {
                if let Some(&new_idx) = cluster_map.get(&old_idx) {
                    let old_fail = nodes[old_idx].fail;
                    if let Some(&new_fail) = cluster_map.get(&old_fail) {
                        new_nodes[new_idx].fail = new_fail;
                    }
                }
            }

            // Restore outputs
            for (old_idx, &new_idx) in &cluster_map {
                let old_node = &nodes[*old_idx];
                new_nodes[new_idx]
                    .output
                    .extend_from_slice(&old_node.output);
            }
            for node in &mut new_nodes {
                node.output.sort();
                node.output.dedup();
            }

            // Restore pattern_index
            for node in &mut new_nodes {
                if !node.output.is_empty() {
                    node.pattern_index = Some(node.output[0]);
                } else {
                    node.pattern_index = None;
                }
            }

            // Reconstruct patterns
            let new_patterns =
                self.reconstruct_patterns(&clusters, &f_lambda, &patterns, &node_to_pattern_map);
            patterns = new_patterns;
            nodes = new_nodes;
        }

        FuzzyAhoCorasick {
            nodes,
            patterns,
            similarity: Cow::Borrowed(similarity),
            limits: self.limits,
            penalties: self.penalties,
            case_insensitive: self.case_insensitive,
        }
    }

    fn reconstruct_patterns(
        &self,
        clusters: &[Vec<usize>],
        f_lambda: &BTreeSet<usize>,
        patterns: &[Pattern],
        node_to_pattern_map: &BTreeMap<usize, usize>,
    ) -> Vec<Pattern> {
        let mut new_patterns = Vec::new();
        let mut seen = BTreeSet::new(); // избегаем дубликатов

        for &cluster_idx in f_lambda {
            let cluster = &clusters[cluster_idx];

            for &old_node_idx in cluster {
                if let Some(&pat_idx) = node_to_pattern_map.get(&old_node_idx) {
                    if pat_idx < patterns.len() {
                        let pattern = &patterns[pat_idx];
                        let key = pattern.custom_unique_id.unwrap_or(pat_idx);

                        if seen.insert(key) {
                            new_patterns.push(pattern.clone());
                        }
                    }
                }
            }
        }

        new_patterns.sort_by_key(|p| p.custom_unique_id);
        new_patterns
    }

    /// Checks if node `new_idx` can be added to `cluster`
    pub(crate) fn nodes_compatible(
        nodes: &[Node],
        cluster: &[usize],
        new_idx: usize,
        lambda: f32,
    ) -> bool {
        fn in_vector(nodes: &[Node], node_idx: usize, all_chars: &[&str]) -> Vec<f32> {
            let mut vec = vec![0.0; all_chars.len()];

            for (i, c) in all_chars.iter().enumerate() {
                let mut total = 0.0;
                for (from_idx, node) in nodes.iter().enumerate() {
                    if let Some(targets) = node.transitions.get(*c) {
                        if targets.contains(&node_idx) {
                            total += nodes[from_idx].weight; // w(src)
                        }
                    }
                }
                vec[i] = total;
            }

            vec
        }

        fn out_vector(nodes: &[Node], node_idx: usize, all_chars: &[&str]) -> Vec<f32> {
            let mut vec = vec![0.0; all_chars.len()];
            let node = &nodes[node_idx];

            for (i, c) in all_chars.iter().enumerate() {
                let mut total = 0.0;
                if let Some(targets) = node.transitions.get(*c) {
                    for &target_idx in targets {
                        total += nodes[target_idx].weight; // w(dst)
                    }
                }
                vec[i] = total;
            }

            vec
        }

        fn vector_diff(a: &[f32], b: &[f32]) -> f32 {
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| (x - y).abs())
                .max_by(|x, y| x.total_cmp(y))
                .unwrap_or(0.0)
        }

        let all_chars: Vec<&str> = nodes
            .iter()
            .flat_map(|node| node.transitions.keys())
            .map(String::as_str)
            .unique()
            .collect();

        for &existing_idx in cluster {
            if nodes[new_idx].pattern_index.is_some() != nodes[existing_idx].pattern_index.is_some()
                || vector_diff(
                    &out_vector(nodes, new_idx, all_chars.as_slice()),
                    &out_vector(nodes, existing_idx, all_chars.as_slice()),
                ) >= lambda
                || vector_diff(
                    &in_vector(nodes, new_idx, all_chars.as_slice()),
                    &in_vector(nodes, existing_idx, all_chars.as_slice()),
                ) >= lambda
            {
                return false;
            }
        }

        true
    }
}

/* -------------------------------------------------------------------------
 *  Default similarity map
 * ---------------------------------------------------------------------- */

/// Singleton that stores the lazily‑initialised vowel/consonant similarity map.
static DEFAULT_SIMILARITY_MAP: LazyLock<BTreeMap<(char, char), f32>> = LazyLock::new(|| {
    let mut map = BTreeMap::new();
    let vowels = ['a', 'e', 'i', 'o', 'u'];
    let consonants = (b'a'..=b'z')
        .map(|b| b as char)
        .filter(|c| !vowels.contains(c))
        .collect::<Vec<_>>();

    // Vowel ↔ vowel similarities.
    for &a in &vowels {
        for &b in &vowels {
            if a != b {
                map.insert((a, b), 0.8);
            }
        }
    }
    // Consonant ↔ consonant similarities.
    for &a in &consonants {
        for &b in &consonants {
            if a != b {
                map.insert((a, b), 0.6);
            }
        }
    }
    map.insert(('o', '0'), 0.8);
    map.insert(('0', 'o'), 0.8);
    map
});
