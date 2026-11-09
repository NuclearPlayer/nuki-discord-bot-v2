use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One search result, as the scripts get it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

/// The maximum length of a provider's message in `ProviderError`.
const MAX_MESSAGE_CHARS: usize = 200;

/// The failure of one web provider. If a provider fails, `WebChain` tries the next provider.
///
/// The error shows only the first line of a provider's message, cut to `MAX_MESSAGE_CHARS`.
/// The rate-limit messages of Firecrawl and Exa are sign-up text, with instructions for agents.
/// That text must not go to the LLM.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("the request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("HTTP {status}: {}", short(message))]
    Status { status: u16, message: String },
    #[error("JSON-RPC error {code}: {}", short(message))]
    Rpc { code: i64, message: String },
    #[error("the tool failed: {}", short(.0))]
    Tool(String),
    #[error("the answer is not valid: {}", short(.0))]
    InvalidAnswer(String),
    #[error("could not read {url}: {}", short(message))]
    Failed { url: String, message: String },
    #[error("no text for {url}")]
    NoContent { url: String },
}

/// Parses the body of a provider's answer. If the body does not parse, returns `InvalidAnswer`.
pub(crate) fn parse_json<T: DeserializeOwned>(body: &str) -> Result<T, ProviderError> {
    serde_json::from_str(body).map_err(|error| ProviderError::InvalidAnswer(error.to_string()))
}

/// Returns the first line of `message`, cut to `MAX_MESSAGE_CHARS`, on a character boundary.
fn short(message: &str) -> &str {
    let line = message.lines().next().unwrap_or_default();
    match line.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((end, _)) => &line[..end],
        None => line,
    }
}

/// A keyless service that searches the web and gives the text of web pages.
pub trait WebProvider: Send + Sync {
    /// The name of the provider in the logs and in the errors.
    fn name(&self) -> &'static str;

    /// Returns the results of a web search for `query`, at most `limit` of them.
    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: u32,
    ) -> BoxFuture<'a, Result<Vec<SearchResult>, ProviderError>>;

    /// Returns the text of the page at `url`. The provider downloads the page, not Nuki.
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, ProviderError>>;
}
