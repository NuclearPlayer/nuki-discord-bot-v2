use std::fmt;

use serde::{Deserialize, Serialize};

/// A GitHub repository, written as `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoId {
    pub owner: String,
    pub name: String,
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("expected a repository in the form owner/name, got {0:?}")]
pub struct ParseRepoIdError(pub String);

impl RepoId {
    pub fn parse(text: &str) -> Result<RepoId, ParseRepoIdError> {
        match text.split_once('/') {
            Some((owner, name)) if is_valid_id(owner, name) => Ok(RepoId {
                owner: owner.to_string(),
                name: name.to_string(),
            }),
            _ => Err(ParseRepoIdError(text.to_string())),
        }
    }
}

fn is_valid_id(owner: &str, name: &str) -> bool {
    !owner.is_empty() && !name.is_empty() && !name.contains('/')
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
        assert_eq!(
            RepoId::parse("nukeop/nuclear"),
            Ok(RepoId {
                owner: "nukeop".to_string(),
                name: "nuclear".to_string(),
            })
        );
    }

    #[test]
    fn parse_rejects_text_without_a_slash() {
        assert_eq!(
            RepoId::parse("nuclear"),
            Err(ParseRepoIdError("nuclear".to_string()))
        );
    }
}
