//! Streaming fuzzy search over a [`Read`] source.
//!
//! Rather than loading the whole haystack, the streaming API consumes the reader incrementally in
//! bounded, overlapping windows. So it runs in **constant memory** regardless of input size, can
//! process data as it arrives (files, sockets, pipes, decompressors), and — as one consequence —
//! handles inputs beyond the ~4 GiB a single
//! [`FuzzyAhoCorasick::search`](crate::FuzzyAhoCorasick::search) call supports (grapheme positions
//! are `u32`). Matches are reported at absolute `u64` byte offsets.
//!
//! Windows overlap by the longest possible match (computed from the patterns and edit limits), so a
//! match spanning a window boundary is never split, and each window "owns" the matches whose start
//! falls in its non-overlap prefix — so every match is emitted exactly once with no cross-window
//! deduplication.
//!
//! Three entry points, all yielding [`StreamMatch`] with absolute offsets:
//! * [`search_stream`](crate::FuzzyAhoCorasick::search_stream) — callback, single-threaded.
//! * [`stream_matches`](crate::FuzzyAhoCorasick::stream_matches) — an [`Iterator`].
//! * [`search_stream_parallel`](crate::FuzzyAhoCorasick::search_stream_parallel) — callback, fanned
//!   across a thread pool.

use crate::{FuzzyAhoCorasick, FuzzyLimits, NumEdits};
use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Arc, Mutex, mpsc};
use unicode_segmentation::UnicodeSegmentation;

/// A match found by the streaming API, with **absolute byte offsets** into the whole stream.
///
/// Unlike [`FuzzyMatch`](crate::FuzzyMatch) — which borrows the haystack — this owns its matched
/// `text`, so it is `Send` and outlives the transient window it was found in. Look the pattern up
/// via `engine.patterns()[pattern_index]` if you need it.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamMatch {
    /// Absolute (stream-wide) inclusive start byte offset.
    pub start: u64,
    /// Absolute (stream-wide) exclusive end byte offset.
    pub end: u64,
    /// Index of the matched pattern.
    pub pattern_index: usize,
    /// Final similarity score.
    pub similarity: f32,
    /// Number of insertions.
    pub insertions: NumEdits,
    /// Number of deletions.
    pub deletions: NumEdits,
    /// Number of substitutions.
    pub substitutions: NumEdits,
    /// Number of swaps (transpositions).
    pub swaps: NumEdits,
    /// Total number of edits.
    pub edits: NumEdits,
    /// The matched slice of the stream (owned).
    pub text: String,
}

/// Default per-window byte target. Window size does not affect throughput (cost is dominated by the
/// per-position search), so this is chosen only to keep the per-window grapheme buffers cache-
/// resident; it grows automatically if the overlap needs more room.
const DEFAULT_WINDOW: usize = 256 * 1024;

/// An owned window handed to the search: covers global bytes `[base, base + text.len())`, and owns
/// the matches whose start byte is `< commit`.
struct StreamWindow {
    base: u64,
    text: String,
    commit: usize,
}

/// Cuts a byte stream into owned, overlapping windows. The commit boundary keeps the last
/// `overlap_graphemes` graphemes so no match is split; it is always a grapheme boundary.
struct WindowReader<R> {
    reader: R,
    buf: Vec<u8>,
    chunk: Vec<u8>,
    base: u64,
    total: u64,
    window: usize,
    overlap_graphemes: usize,
    done: bool,
}

impl<R: Read> WindowReader<R> {
    fn new(reader: R, window: usize, overlap_graphemes: usize) -> Self {
        Self {
            reader,
            buf: Vec::with_capacity(window),
            chunk: vec![0u8; 64 * 1024],
            base: 0,
            total: 0,
            window,
            overlap_graphemes,
            done: false,
        }
    }

    fn next_window(&mut self) -> io::Result<Option<StreamWindow>> {
        if self.done {
            return Ok(None);
        }
        loop {
            while self.buf.len() < self.window {
                let n = self.reader.read(&mut self.chunk)?;
                if n == 0 {
                    break;
                }
                self.buf.extend_from_slice(&self.chunk[..n]);
                self.total += n as u64;
            }
            let eof = self.buf.len() < self.window;

            // Search only the valid-UTF-8 prefix; a trailing partial code point waits for more bytes.
            let valid = match std::str::from_utf8(&self.buf) {
                Ok(s) => s.len(),
                Err(e) => e.valid_up_to(),
            };
            let text = std::str::from_utf8(&self.buf[..valid]).expect("valid_up_to prefix");

            if eof {
                self.done = true;
                return Ok(Some(StreamWindow {
                    base: self.base,
                    commit: text.len(),
                    text: text.to_owned(),
                }));
            }

            // Commit boundary: keep the last `overlap_graphemes` graphemes so no match is split.
            // Walk graphemes from the *end* — O(overlap), not O(window): re-segmenting the whole
            // window here (on the single producer thread) would serialize the parallel search.
            let commit = match text
                .grapheme_indices(true)
                .rev()
                .nth(self.overlap_graphemes - 1)
            {
                Some((off, _)) if off > 0 => off,
                _ => {
                    // Fewer than overlap_graphemes+1 graphemes: too small to make progress (huge
                    // clusters, or a very long longest-match). Grow the window and read more.
                    self.window += self.window.max(64 * 1024);
                    continue;
                }
            };
            let out = StreamWindow {
                base: self.base,
                text: text.to_owned(),
                commit,
            };
            self.buf.drain(..commit);
            self.base += commit as u64;
            return Ok(Some(out));
        }
    }
}

/// Iterator returned by [`FuzzyAhoCorasick::stream_matches`].
///
/// Yields `io::Result<StreamMatch>`: an `Err` is produced once if the underlying reader fails, after
/// which iteration ends.
pub struct StreamMatches<'a, R> {
    engine: &'a FuzzyAhoCorasick,
    reader: WindowReader<R>,
    threshold: f32,
    pending: VecDeque<StreamMatch>,
    errored: bool,
}

impl<R: Read> Iterator for StreamMatches<'_, R> {
    type Item = io::Result<StreamMatch>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(m) = self.pending.pop_front() {
                return Some(Ok(m));
            }
            if self.errored {
                return None;
            }
            match self.reader.next_window() {
                Ok(Some(w)) => {
                    let mut batch = Vec::new();
                    self.engine.window_matches(
                        &w.text,
                        w.base,
                        w.commit,
                        self.threshold,
                        &mut batch,
                    );
                    self.pending.extend(batch);
                }
                Ok(None) => return None,
                Err(e) => {
                    self.errored = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

impl FuzzyAhoCorasick {
    /// An upper bound, in graphemes, on the longest span a single match can cover: the longest
    /// pattern plus what the edit budget can add (each edit can extend the matched span, and a
    /// multi-character mapping can consume several haystack graphemes). This is the amount of
    /// overlap the streaming windows need so no match is ever split at a boundary; it is exposed so
    /// callers can implement their own windowing.
    #[must_use]
    pub fn max_match_graphemes(&self) -> usize {
        let max_pattern = self
            .patterns
            .iter()
            .map(|p| p.grapheme_len)
            .max()
            .unwrap_or(0);
        // Longest haystack side of any mapping (a mapping may consume more haystack graphemes than
        // it does pattern graphemes); at least 1 so a plain insertion counts as one grapheme.
        let max_mapping_haystack = self
            .mappings
            .values()
            .flat_map(|m| m.iter())
            .map(|mt| mt.haystack.len())
            .max()
            .unwrap_or(1)
            .max(1);
        let edits_of = |l: &FuzzyLimits| -> usize {
            l.edits.map_or_else(
                || {
                    l.insertions.unwrap_or(0) as usize
                        + l.deletions.unwrap_or(0) as usize
                        + l.substitutions.unwrap_or(0) as usize
                        + l.swaps.unwrap_or(0) as usize
                },
                |e| e as usize,
            )
        };
        let max_edits = self
            .patterns
            .iter()
            .map(|p| {
                p.limits
                    .as_ref()
                    .or(self.limits.as_ref())
                    .map_or(0, edits_of)
            })
            .max()
            .unwrap_or(0);
        max_pattern + max_edits * max_mapping_haystack
    }

    /// Grapheme overlap the windows carry (`max_match_graphemes` plus a one-grapheme margin).
    fn stream_overlap(&self) -> usize {
        self.max_match_graphemes() + 1
    }

    /// Convert the window-local matches at `[base ..]` into owned [`StreamMatch`]es, keeping only
    /// those the window owns (start byte `< commit`) so each match is emitted exactly once.
    fn window_matches(
        &self,
        text: &str,
        base: u64,
        commit: usize,
        threshold: f32,
        out: &mut Vec<StreamMatch>,
    ) {
        for m in self.search_non_overlapping(text, threshold).iter() {
            if m.start < commit {
                out.push(StreamMatch {
                    start: base + m.start as u64,
                    end: base + m.end as u64,
                    pattern_index: m.pattern_index,
                    similarity: m.similarity,
                    insertions: m.insertions,
                    deletions: m.deletions,
                    substitutions: m.substitutions,
                    swaps: m.swaps,
                    edits: m.edits,
                    text: m.text.to_owned(),
                });
            }
        }
    }

    /// Search a byte stream of any size, invoking `on_match` for each match with absolute offsets.
    /// Single-threaded. Returns the total number of bytes read from `reader`.
    ///
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["needle"]);
    /// let mut hits = 0;
    /// // "neeedle" has an extra 'e' (one insertion).
    /// engine.search_stream("hay neeedle hay".as_bytes(), 0.8, |m| {
    ///     assert_eq!(m.pattern_index, 0);
    ///     hits += 1;
    /// }).unwrap();
    /// assert_eq!(hits, 1);
    /// ```
    ///
    /// # Errors
    /// Propagates any [`io::Error`] from `reader`.
    pub fn search_stream<R: Read>(
        &self,
        reader: R,
        threshold: f32,
        mut on_match: impl FnMut(StreamMatch),
    ) -> io::Result<u64> {
        let mut wr = WindowReader::new(reader, DEFAULT_WINDOW, self.stream_overlap());
        let mut batch = Vec::new();
        while let Some(w) = wr.next_window()? {
            batch.clear();
            self.window_matches(&w.text, w.base, w.commit, threshold, &mut batch);
            for m in batch.drain(..) {
                on_match(m);
            }
        }
        Ok(wr.total)
    }

    /// Stream matches as an [`Iterator`] of `io::Result<StreamMatch>`. Single-threaded and lazy —
    /// windows are read and searched on demand as the iterator is advanced.
    ///
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["needle"]);
    /// let found: Vec<_> = engine
    ///     .stream_matches("a neeedle b needlee c".as_bytes(), 0.8)
    ///     .map(Result::unwrap)
    ///     .collect();
    /// assert_eq!(found.len(), 2);
    /// assert!(found[0].start < found[1].start);
    /// ```
    pub fn stream_matches<R: Read>(&self, reader: R, threshold: f32) -> StreamMatches<'_, R> {
        StreamMatches {
            engine: self,
            reader: WindowReader::new(reader, DEFAULT_WINDOW, self.stream_overlap()),
            threshold,
            pending: VecDeque::new(),
            errored: false,
        }
    }

    /// Parallel [`search_stream`](Self::search_stream): a single producer cuts windows while
    /// `threads` workers run the (CPU-bound) search in parallel, sharing this immutable engine.
    /// `on_match` is invoked on the calling thread as results arrive (in arbitrary order), so it
    /// needs no synchronization. Uses `std::thread` only — no runtime dependencies. Returns the
    /// total number of bytes read from `reader`.
    ///
    /// `threads` is clamped to at least 1; pass
    /// [`std::thread::available_parallelism`] for "all cores".
    ///
    /// # Errors
    /// Propagates any [`io::Error`] from `reader`.
    ///
    /// # Panics
    /// Propagates a panic from a worker or the producer thread (e.g. an out-of-memory abort while
    /// building a window), re-raised on the calling thread.
    pub fn search_stream_parallel<R: Read + Send>(
        &self,
        reader: R,
        threshold: f32,
        threads: usize,
        mut on_match: impl FnMut(StreamMatch),
    ) -> io::Result<u64> {
        let threads = threads.max(1);
        std::thread::scope(|scope| {
            // Bounded so the producer can't read the whole stream ahead of the workers.
            let (work_tx, work_rx) = mpsc::sync_channel::<StreamWindow>(threads * 2);
            let work_rx = Arc::new(Mutex::new(work_rx));
            let (res_tx, res_rx) = mpsc::channel::<Vec<StreamMatch>>();

            for _ in 0..threads {
                let work_rx = Arc::clone(&work_rx);
                let res_tx = res_tx.clone();
                scope.spawn(move || {
                    loop {
                        // Hold the lock only for the (fast) recv, never during the search.
                        let Ok(w) = work_rx.lock().unwrap().recv() else {
                            break;
                        };
                        let mut out = Vec::new();
                        self.window_matches(&w.text, w.base, w.commit, threshold, &mut out);
                        if res_tx.send(out).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(res_tx); // only the workers' clones keep the results channel open

            let producer = scope.spawn(move || -> io::Result<u64> {
                let mut wr = WindowReader::new(reader, DEFAULT_WINDOW, self.stream_overlap());
                while let Some(w) = wr.next_window()? {
                    if work_tx.send(w).is_err() {
                        break; // workers gone
                    }
                }
                Ok(wr.total)
                // work_tx dropped here -> workers observe the channel close and exit.
            });

            for batch in res_rx {
                for m in batch {
                    on_match(m);
                }
            }
            producer.join().expect("stream producer panicked")
        })
    }
}
