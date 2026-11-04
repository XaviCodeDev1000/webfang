//! DOM inspector implementations — infrastructure layer.
//!
//! Provides two implementations of [`DomInspectorPort`]:
//! - [`NoOpInspector`] — Null Object pattern, zero overhead on happy path.
//! - [`DefaultDomInspector`] — walks the DOM tree and computes Jaro-Winkler
//!   similarity scores for selector suggestions.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::domain::{DomInspectorPort, DomStructureReport, SelectorSuggestion};

// ---------------------------------------------------------------------------
// NoOpInspector
// ---------------------------------------------------------------------------

/// No-op DOM inspector — returns empty reports with zero overhead.
///
/// Null Object pattern: used when diagnostics are not needed (non-MCP paths
/// or when an inspector is not configured in `McpState`).
#[derive(Debug, Clone, Default)]
pub struct NoOpInspector;

impl DomInspectorPort for NoOpInspector {
    fn inspect(&self, _document: &scraper::Html) -> DomStructureReport {
        DomStructureReport::default()
    }

    fn suggest(
        &self,
        _document: &scraper::Html,
        _failed_selector: &str,
    ) -> Vec<SelectorSuggestion> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// DefaultDomInspector
// ---------------------------------------------------------------------------

/// Default DOM inspector — walks the DOM tree and computes similarity scores.
///
/// Uses `scraper` for DOM traversal and `strsim::jaro_winkler` for similarity.
/// Node-count cap prevents hangs on very large DOMs (default: 10,000).
pub struct DefaultDomInspector {
    node_count_cap: usize,
    similarity_threshold: f64,
}

impl DefaultDomInspector {
    /// Default node-count cap (10,000 elements).
    pub const DEFAULT_NODE_CAP: usize = 10_000;

    /// Default Jaro-Winkler similarity threshold (0.6).
    pub const DEFAULT_THRESHOLD: f64 = 0.6;

    /// Create a new `DefaultDomInspector` with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            node_count_cap: Self::DEFAULT_NODE_CAP,
            similarity_threshold: Self::DEFAULT_THRESHOLD,
        }
    }

    /// Set the maximum number of elements to inspect before truncating.
    #[must_use]
    pub fn with_node_cap(mut self, cap: usize) -> Self {
        self.node_count_cap = cap;
        self
    }

    /// Set the minimum Jaro-Winkler similarity score for suggestions.
    #[must_use]
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.similarity_threshold = threshold;
        self
    }

    /// Collect candidate selectors from the DOM.
    ///
    /// Returns a deduplicated `Vec<String>` of bare tag names, `.class`
    /// selectors, and `#id` selectors.
    fn collect_candidates(document: &scraper::Html) -> Vec<String> {
        let selector = match scraper::Selector::parse("*") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut candidates: Vec<String> = Vec::new();

        for element_ref in document.select(&selector) {
            let el = element_ref.value();

            // Tag name candidate
            let tag = el.name().to_owned();
            if seen.insert(tag.clone()) {
                candidates.push(tag);
            }

            // Class selector candidates
            for class in el.classes() {
                let class_sel = format!(".{class}");
                if seen.insert(class_sel.clone()) {
                    candidates.push(class_sel);
                }
            }

            // ID selector candidate
            if let Some(id) = el.id() {
                let id_sel = format!("#{id}");
                if seen.insert(id_sel.clone()) {
                    candidates.push(id_sel);
                }
            }
        }

        candidates
    }
}

impl Default for DefaultDomInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl DomInspectorPort for DefaultDomInspector {
    fn inspect(&self, document: &scraper::Html) -> DomStructureReport {
        let selector = match scraper::Selector::parse("*") {
            Ok(s) => s,
            Err(_) => return DomStructureReport::default(),
        };

        let mut element_count: usize = 0;
        let mut truncated = false;
        let mut max_depth: usize = 0;
        let mut tag_counts: HashMap<String, usize> = HashMap::new();
        let mut class_counts: HashMap<String, usize> = HashMap::new();
        let mut id_counts: HashMap<String, usize> = HashMap::new();

        for element_ref in document.select(&selector) {
            if element_count >= self.node_count_cap {
                truncated = true;
                break;
            }

            let el = element_ref.value();

            element_count += 1;

            // Depth = number of ancestors (including the Document root node).
            let depth = element_ref.ancestors().count();
            if depth > max_depth {
                max_depth = depth;
            }

            // Tag count
            *tag_counts.entry(el.name().to_owned()).or_insert(0) += 1;

            // Class counts
            for class in el.classes() {
                *class_counts.entry(class.to_string()).or_insert(0) += 1;
            }

            // ID count
            if let Some(id) = el.id() {
                *id_counts.entry(id.to_owned()).or_insert(0) += 1;
            }
        }

        // Sort common_classes descending by frequency, then alphabetically for
        // deterministic output when frequencies are tied. Top 10.
        let mut common_classes: Vec<(String, usize)> = class_counts.into_iter().collect();
        common_classes.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        common_classes.truncate(10);

        // Sort common_ids descending by frequency, then alphabetically. Top 5.
        let mut common_ids: Vec<(String, usize)> = id_counts.into_iter().collect();
        common_ids.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        common_ids.truncate(5);

        DomStructureReport {
            element_count,
            truncated,
            tag_counts,
            max_depth,
            common_classes,
            common_ids,
        }
    }

    fn suggest(&self, document: &scraper::Html, failed_selector: &str) -> Vec<SelectorSuggestion> {
        let candidates = Self::collect_candidates(document);

        let mut suggestions: Vec<SelectorSuggestion> = candidates
            .into_iter()
            .map(|candidate| {
                let score = strsim::jaro_winkler(failed_selector, &candidate);
                SelectorSuggestion {
                    selector: candidate,
                    score,
                }
            })
            .filter(|s| s.score >= self.similarity_threshold)
            .collect();

        // Sort by score descending. Use unwrap_or(Equal) for NaN safety.
        suggestions
            .sort_unstable_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        // Truncate to top 5.
        suggestions.truncate(5);

        suggestions
    }
}
