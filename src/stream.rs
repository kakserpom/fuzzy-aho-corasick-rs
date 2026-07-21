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
//! Three search entry points, all yielding [`StreamMatch`] with absolute offsets:
//! * [`search_stream`](crate::FuzzyAhoCorasick::search_stream) — callback, single-threaded.
//! * [`stream_matches`](crate::FuzzyAhoCorasick::stream_matches) — an [`Iterator`].
//! * [`search_stream_parallel`](crate::FuzzyAhoCorasick::search_stream_parallel) — callback, fanned
//!   across a thread pool.
//!
//! And a streaming find-and-replace that writes the transformed stream to a [`Write`] sink:
//! * [`replace_stream`](crate::FuzzyAhoCorasick::replace_stream) — callback, single-threaded.
//! * [`replace_stream_parallel`](crate::FuzzyAhoCorasick::replace_stream_parallel) — parallel search,
//!   output reassembled in stream order on the calling thread.

use crate::{FuzzyAhoCorasick, FuzzyLimits, FuzzyMatch, NumEdits};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
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

    /// Streaming fuzzy find-and-replace: read from `reader`, write the transformed stream to
    /// `writer` in **constant memory**. For each non-overlapping match above `threshold`, `callback`
    /// is invoked — returning `Some(replacement)` substitutes the matched span, `None` keeps the
    /// original text — and everything between matches is copied through verbatim. Returns the number
    /// of bytes written to `writer`.
    ///
    /// This is the streaming counterpart of [`replace`](Self::replace). Matches are selected per
    /// window (as in the streaming search), so at a window boundary overlap is resolved
    /// left-to-right — the earlier-starting match wins — rather than by the global ranking a
    /// whole-input [`replace`](Self::replace) would use. For inputs where matches are separated by
    /// non-matching text the two agree exactly.
    ///
    /// The replacement type is independent of the match, so it may borrow external data (e.g. a
    /// table of replacements) but **not** the transient matched text; return an owned `String` if you
    /// need to derive the replacement from `m.text`.
    ///
    /// Wrap `writer` in a [`BufWriter`](std::io::BufWriter) for best throughput.
    ///
    /// ```
    /// use fuzzy_aho_corasick::{FuzzyAhoCorasickBuilder, FuzzyLimits};
    /// let engine = FuzzyAhoCorasickBuilder::new()
    ///     .fuzzy(FuzzyLimits::new().edits(1))
    ///     .case_insensitive(true)
    ///     .build(["needle"]);
    /// let mut out = Vec::new();
    /// // "neeedle" has one extra 'e' (an insertion); it is replaced, the rest copied through.
    /// engine
    ///     .replace_stream("a neeedle b".as_bytes(), &mut out, |_m| Some("X"), 0.8)
    ///     .unwrap();
    /// assert_eq!(String::from_utf8(out).unwrap(), "a X b");
    /// ```
    ///
    /// # Errors
    /// Propagates any [`io::Error`] from `reader` or `writer`.
    pub fn replace_stream<R, W, F, S>(
        &self,
        reader: R,
        mut writer: W,
        mut callback: F,
        threshold: f32,
    ) -> io::Result<u64>
    where
        R: Read,
        W: Write,
        F: FnMut(&FuzzyMatch) -> Option<S>,
        S: AsRef<str>,
    {
        let mut wr = WindowReader::new(reader, DEFAULT_WINDOW, self.stream_overlap());
        let mut cursor = ReplaceCursor::default();
        while let Some(w) = wr.next_window()? {
            let matches = self.window_replace_matches(&w.text, w.commit, threshold);
            cursor.emit_window(
                &mut writer,
                &mut callback,
                w.base,
                &w.text,
                w.commit,
                &matches,
            )?;
        }
        Ok(cursor.written)
    }

    /// The matches a window owns for replacement: non-overlapping, starting before `commit`, sorted
    /// by position. Returned owned so the parallel path can move them across threads.
    fn window_replace_matches<'a>(
        &'a self,
        text: &'a str,
        commit: usize,
        threshold: f32,
    ) -> Vec<FuzzyMatch<'a>> {
        let mut matches: Vec<FuzzyMatch> = self
            .search_non_overlapping(text, threshold)
            .into_iter()
            .filter(|m| m.start < commit)
            .collect();
        matches.sort_unstable_by_key(|m| (m.start, m.end));
        matches
    }

    /// Parallel [`replace_stream`](Self::replace_stream): a producer cuts windows, `threads` workers
    /// run the (CPU-bound) search in parallel, and this thread reassembles the output **in stream
    /// order**, calling `callback` and writing to `writer`. Because output is inherently ordered,
    /// only the search is parallelised; the callback and writer stay on the calling thread (no
    /// `Send`/`Sync` bounds, exactly as in the single-threaded form). Returns the bytes written.
    ///
    /// Semantics are identical to [`replace_stream`](Self::replace_stream). `threads` is clamped to
    /// at least 1; pass [`std::thread::available_parallelism`] for "all cores".
    ///
    /// # Errors
    /// Propagates any [`io::Error`] from `reader` or `writer`.
    ///
    /// # Panics
    /// Propagates a panic from a worker or the producer thread, re-raised on the calling thread.
    pub fn replace_stream_parallel<R, W, F, S>(
        &self,
        reader: R,
        mut writer: W,
        threads: usize,
        mut callback: F,
        threshold: f32,
    ) -> io::Result<u64>
    where
        R: Read + Send,
        W: Write,
        F: FnMut(&FuzzyMatch) -> Option<S>,
        S: AsRef<str>,
    {
        let threads = threads.max(1);
        let cancel = Arc::new(AtomicBool::new(false));
        std::thread::scope(|scope| {
            // Bounded so the producer can't read the whole stream ahead of the workers.
            let (work_tx, work_rx) = mpsc::sync_channel::<(u64, StreamWindow)>(threads * 2);
            let work_rx = Arc::new(Mutex::new(work_rx));
            // Unbounded: with in-order reassembly a bounded results channel can deadlock (the worker
            // holding the next-needed window blocks on a full channel while the collector waits for
            // exactly that window).
            let (res_tx, res_rx) = mpsc::channel::<ReplaceResult>();

            for _ in 0..threads {
                let work_rx = Arc::clone(&work_rx);
                let res_tx = res_tx.clone();
                scope.spawn(move || {
                    loop {
                        let Ok((seq, w)) = work_rx.lock().unwrap().recv() else {
                            break;
                        };
                        let matches = self
                            .window_replace_matches(&w.text, w.commit, threshold)
                            .iter()
                            .map(OwnedMatch::from)
                            .collect();
                        let res = ReplaceResult {
                            seq,
                            base: w.base,
                            text: w.text,
                            commit: w.commit,
                            matches,
                        };
                        if res_tx.send(res).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(res_tx); // only the workers' clones keep the results channel open

            let producer = {
                let cancel = Arc::clone(&cancel);
                scope.spawn(move || -> io::Result<u64> {
                    let mut wr = WindowReader::new(reader, DEFAULT_WINDOW, self.stream_overlap());
                    let mut seq = 0u64;
                    while !cancel.load(Ordering::Relaxed) {
                        let Some(w) = wr.next_window()? else { break };
                        if work_tx.send((seq, w)).is_err() {
                            break; // workers gone
                        }
                        seq += 1;
                    }
                    Ok(wr.total)
                    // work_tx dropped here -> workers observe the channel close and exit.
                })
            };

            // Collector: reassemble windows in producer order, emitting output as each becomes ready.
            let mut cursor = ReplaceCursor::default();
            let mut next_seq = 0u64;
            let mut pending: HashMap<u64, ReplaceResult> = HashMap::new();
            let mut write_err: Option<io::Error> = None;
            'collect: for res in res_rx {
                pending.insert(res.seq, res);
                while let Some(r) = pending.remove(&next_seq) {
                    let matches: Vec<FuzzyMatch> = r
                        .matches
                        .iter()
                        .map(|om| om.to_match(self, &r.text))
                        .collect();
                    if let Err(e) = cursor.emit_window(
                        &mut writer,
                        &mut callback,
                        r.base,
                        &r.text,
                        r.commit,
                        &matches,
                    ) {
                        write_err = Some(e);
                        cancel.store(true, Ordering::Relaxed); // stop the producer promptly
                        break 'collect;
                    }
                    next_seq += 1;
                }
            }

            let read_result = producer.join().expect("stream producer panicked");
            match write_err {
                Some(e) => Err(e),
                None => read_result.map(|_| cursor.written),
            }
        })
    }
}

/// Tracks output progress across windows for streaming replace. `emitted` is the absolute byte
/// offset up to which output has been written; it is monotonic and always `>=` the current window's
/// base, so `emitted - base` indexes into that window's text.
#[derive(Default)]
struct ReplaceCursor {
    emitted: u64,
    written: u64,
}

impl ReplaceCursor {
    /// Emit one window's output: verbatim runs interleaved with replacements, then verbatim up to
    /// the commit boundary. `matches` must be this window's owned matches (start `< commit`, sorted
    /// by position). Cross-window overlap is resolved by skipping any match already covered.
    fn emit_window<W, F, S>(
        &mut self,
        writer: &mut W,
        callback: &mut F,
        base: u64,
        text: &str,
        commit: usize,
        matches: &[FuzzyMatch],
    ) -> io::Result<()>
    where
        W: Write,
        F: FnMut(&FuzzyMatch) -> Option<S>,
        S: AsRef<str>,
    {
        let bytes = text.as_bytes();
        for m in matches {
            let match_start = base + m.start as u64;
            if match_start < self.emitted {
                // Overlaps a span already written (a match from an earlier window that extended past
                // its commit boundary); the earlier match won, so skip this one.
                continue;
            }
            // Verbatim run between the previous emission and this match.
            if self.emitted < match_start {
                let lo = (self.emitted - base) as usize;
                writer.write_all(&bytes[lo..m.start])?;
                self.written += (m.start - lo) as u64;
            }
            // The replacement, or the original text when the callback declines.
            if let Some(repl) = callback(m) {
                let repl = repl.as_ref().as_bytes();
                writer.write_all(repl)?;
                self.written += repl.len() as u64;
            } else {
                writer.write_all(&bytes[m.start..m.end])?;
                self.written += (m.end - m.start) as u64;
            }
            self.emitted = base + m.end as u64;
        }

        // Flush verbatim up to this window's commit boundary (the whole window on EOF, where
        // commit == text.len()). Skipped if a replacement already carried `emitted` past it.
        let commit_abs = base + commit as u64;
        if self.emitted < commit_abs {
            let lo = (self.emitted - base) as usize;
            writer.write_all(&bytes[lo..commit])?;
            self.written += (commit - lo) as u64;
            self.emitted = commit_abs;
        }
        Ok(())
    }
}

/// A window's owned matches shipped from a worker to the collector, tagged with the window's
/// producer order (`seq`) so output can be reassembled in stream order.
struct ReplaceResult {
    seq: u64,
    base: u64,
    text: String,
    commit: usize,
    matches: Vec<OwnedMatch>,
}

/// The scalar fields of a [`FuzzyMatch`], owned so it can cross a thread boundary. The borrowed
/// `pattern` and `text` are re-attached on the collector via [`OwnedMatch::to_match`].
struct OwnedMatch {
    start: usize,
    end: usize,
    pattern_index: usize,
    similarity: f32,
    insertions: NumEdits,
    deletions: NumEdits,
    substitutions: NumEdits,
    swaps: NumEdits,
    edits: NumEdits,
    #[cfg(debug_assertions)]
    notes: Vec<String>,
}

impl From<&FuzzyMatch<'_>> for OwnedMatch {
    fn from(m: &FuzzyMatch<'_>) -> Self {
        Self {
            start: m.start,
            end: m.end,
            pattern_index: m.pattern_index,
            similarity: m.similarity,
            insertions: m.insertions,
            deletions: m.deletions,
            substitutions: m.substitutions,
            swaps: m.swaps,
            edits: m.edits,
            #[cfg(debug_assertions)]
            notes: m.notes.clone(),
        }
    }
}

impl OwnedMatch {
    /// Rebuild a borrowed [`FuzzyMatch`] over the window `text` and `engine`'s pattern table.
    fn to_match<'a>(&self, engine: &'a FuzzyAhoCorasick, text: &'a str) -> FuzzyMatch<'a> {
        FuzzyMatch {
            insertions: self.insertions,
            deletions: self.deletions,
            substitutions: self.substitutions,
            swaps: self.swaps,
            edits: self.edits,
            pattern_index: self.pattern_index,
            pattern: &engine.patterns[self.pattern_index],
            start: self.start,
            end: self.end,
            similarity: self.similarity,
            text: &text[self.start..self.end],
            #[cfg(debug_assertions)]
            notes: self.notes.clone(),
        }
    }
}
