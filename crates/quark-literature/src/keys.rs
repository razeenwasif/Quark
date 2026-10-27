//! The keys some sources give more to: Semantic Scholar's and OpenAlex's,
//! and the address Crossref's polite pool wants. Read from the person's own
//! environment by whoever starts a search, never stored, never printed.

use std::collections::BTreeMap;

use crate::model::LiteratureSource;

pub use crate::model::KeyState;

/// The environment variables they are read from.
pub const NAMES: [&str; 3] = ["SEMANTIC_SCHOLAR_API_KEY", "OPENALEX_API_KEY", "CROSSREF_MAILTO"];

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Keys {
    pub semantic_scholar: Option<String>,
    pub openalex: Option<String>,
    /// An address for Crossref's polite pool: the person's, so only when they give it.
    pub mailto: Option<String>,
}

impl Keys {
    /// From variables as an environment has them; an empty one is none.
    pub fn from_vars(vars: &BTreeMap<String, String>) -> Keys {
        let get = |name: &str| vars.get(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Keys { semantic_scholar: get(NAMES[0]), openalex: get(NAMES[1]), mailto: get(NAMES[2]) }
    }

    /// Whether `source` has its key. arXiv takes none.
    pub fn state(&self, source: LiteratureSource) -> KeyState {
        let found = match source {
            LiteratureSource::Arxiv => return KeyState::NotNeeded,
            LiteratureSource::SemanticScholar => self.semantic_scholar.is_some(),
            LiteratureSource::OpenAlex => self.openalex.is_some(),
        };
        if found {
            KeyState::Found
        } else {
            KeyState::Missing
        }
    }
}

/// Never the keys themselves: whether each is there.
impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let set = |v: &Option<String>| if v.is_some() { "set" } else { "none" };
        f.debug_struct("Keys")
            .field("semantic_scholar", &set(&self.semantic_scholar))
            .field("openalex", &set(&self.openalex))
            .field("mailto", &set(&self.mailto))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_by_name_and_never_printed() {
        let vars: BTreeMap<String, String> =
            [("SEMANTIC_SCHOLAR_API_KEY", " s2-secret "), ("OPENALEX_API_KEY", ""), ("OTHER", "x")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let keys = Keys::from_vars(&vars);
        assert_eq!(keys.semantic_scholar.as_deref(), Some("s2-secret"));
        assert_eq!(keys.openalex, None, "an empty one is none");
        assert_eq!(keys.state(LiteratureSource::OpenAlex), KeyState::Missing);
        assert_eq!(keys.state(LiteratureSource::Arxiv), KeyState::NotNeeded);
        assert!(!format!("{keys:?}").contains("s2-secret"));
    }
}
