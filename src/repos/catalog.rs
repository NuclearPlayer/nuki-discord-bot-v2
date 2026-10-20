use std::fmt;

use serde::{Deserialize, Serialize};

/// The repository of the Nuclear player. The bot clones it at start.
pub const NUCLEAR_REPO: &str = "nukeop/nuclear";

/// A GitHub repository, written as `owner/name`. The owner and the name each contain
/// only `[A-Za-z0-9._-]`, and are not empty, `.`, or `..`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoId {
    owner: String,
    name: String,
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("expected owner/name with only letters, digits, '.', '-' or '_', got {0:?}")]
pub struct ParseRepoIdError(pub String);

impl RepoId {
    pub fn parse(text: &str) -> Result<RepoId, ParseRepoIdError> {
        match text.split_once('/') {
            Some((owner, name)) if is_segment(owner) && is_segment(name) => Ok(RepoId {
                owner: owner.to_string(),
                name: name.to_string(),
            }),
            _ => Err(ParseRepoIdError(text.to_string())),
        }
    }

    /// The repository of the Nuclear player, `NUCLEAR_REPO`.
    pub fn nuclear() -> RepoId {
        RepoId::parse(NUCLEAR_REPO).expect("NUCLEAR_REPO is a valid repository")
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

fn is_segment(text: &str) -> bool {
    !matches!(text, "" | "." | "..") && text.chars().all(is_name_char)
}

/// The characters accepted in an owner or a repository name.
fn is_name_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
}

impl fmt::Display for RepoId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.owner, self.name)
    }
}

impl TryFrom<String> for RepoId {
    type Error = ParseRepoIdError;

    fn try_from(text: String) -> Result<RepoId, ParseRepoIdError> {
        RepoId::parse(&text)
    }
}

impl From<RepoId> for String {
    fn from(id: RepoId) -> String {
        id.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub id: RepoId,
    pub description: Option<String>,
    pub clone_url: String,
}

/// The repositories that the bot can clone, in the sequence that `list_repos` shows them.
#[derive(Debug, Clone)]
pub struct RepoCatalog {
    entries: Vec<CatalogEntry>,
}

impl RepoCatalog {
    pub fn new(entries: Vec<CatalogEntry>) -> RepoCatalog {
        RepoCatalog { entries }
    }

    pub fn get(&self, id: &RepoId) -> Option<&CatalogEntry> {
        self.entries.iter().find(|entry| &entry.id == id)
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_owner_and_name() {
        let id = RepoId::parse("nukeop/nuclear").unwrap();

        assert_eq!((id.owner(), id.name()), ("nukeop", "nuclear"));
    }

    #[test]
    fn parse_accepts_dots_dashes_and_underscores_in_a_segment() {
        let id = RepoId::parse("Nuclear-Player_2/nuclear.js").unwrap();

        assert_eq!((id.owner(), id.name()), ("Nuclear-Player_2", "nuclear.js"));
    }

    #[test]
    fn parse_rejects_text_that_is_not_two_valid_segments() {
        let inputs = [
            "nuclear",
            "nukeop/nuclear/releases",
            "/nuclear",
            "nukeop/",
            "./nuclear",
            "nukeop/.",
            "../nuclear",
            "nukeop/..",
            "nukeop/nuclear is:issue",
            "repo:nukeop/nuclear",
            "nukeop/nuclear?x=",
        ];

        let results: Vec<_> = inputs
            .iter()
            .map(|text| (*text, RepoId::parse(text)))
            .collect();

        let expected: Vec<_> = inputs
            .iter()
            .map(|text| (*text, Err(ParseRepoIdError(text.to_string()))))
            .collect();
        assert_eq!(results, expected);
    }

    #[test]
    fn a_parse_error_tells_the_accepted_characters() {
        let error = RepoId::parse("nukeop/nuclear?x=").unwrap_err();

        assert_eq!(
            error.to_string(),
            "expected owner/name with only letters, digits, '.', '-' or '_', got \"nukeop/nuclear?x=\""
        );
    }
}
