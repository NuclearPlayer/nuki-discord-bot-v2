use futures::future::BoxFuture;
use thiserror::Error;

use crate::web::provider::{ProviderError, SearchResult, WebProvider};

/// The maximum length of the content of a search result. Exa and Parallel send 2 to 25 KB of
/// text for each result. Without the cut, the tool result goes past the output limit of the
/// sandbox.
const MAX_CONTENT_CHARS: usize = 500;

/// The web providers in the sequence that Nuki tries them.
pub struct WebChain {
    providers: Vec<Box<dyn WebProvider>>,
}

/// The error of one provider in a `ChainError`.
#[derive(Debug, Error)]
#[error("{provider}: {error}")]
pub struct ProviderFailure {
    provider: &'static str,
    error: ProviderError,
}

/// The error when all providers failed. It holds the error of each provider, in sequence.
#[derive(Debug, Error)]
#[error("all web providers failed: {}", joined(failures))]
pub struct ChainError {
    failures: Vec<ProviderFailure>,
}

fn joined(failures: &[ProviderFailure]) -> String {
    failures
        .iter()
        .map(ProviderFailure::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

impl WebChain {
    pub fn new(providers: Vec<Box<dyn WebProvider>>) -> Self {
        Self { providers }
    }

    /// Returns the search results of the first provider that answers. Cuts the content of each
    /// result to `MAX_CONTENT_CHARS`.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>, ChainError> {
        let results = self
            .first_answer("search", |provider| provider.search(query, limit))
            .await?;
        Ok(results.into_iter().map(with_short_content).collect())
    }

    /// Returns the page text of the first provider that answers.
    pub async fn fetch(&self, url: &str) -> Result<String, ChainError> {
        self.first_answer("fetch", |provider| provider.fetch(url))
            .await
    }

    /// Sends the request of `call` to each provider in sequence, and returns the first answer.
    /// Logs the provider that answered at `info`, and each provider that failed at `warn`.
    /// `operation` is the name of the request in the logs.
    async fn first_answer<'a, T>(
        &'a self,
        operation: &'static str,
        call: impl Fn(&'a dyn WebProvider) -> BoxFuture<'a, Result<T, ProviderError>>,
    ) -> Result<T, ChainError> {
        let mut failures = Vec::new();
        for provider in &self.providers {
            let name = provider.name();
            match call(provider.as_ref()).await {
                Ok(answer) => {
                    tracing::info!(provider = name, operation, "the web provider answered");
                    return Ok(answer);
                }
                Err(error) => {
                    tracing::warn!(
                        provider = name,
                        operation,
                        %error,
                        "the web provider failed"
                    );
                    failures.push(ProviderFailure {
                        provider: name,
                        error,
                    });
                }
            }
        }
        Err(ChainError { failures })
    }
}

/// Cuts `content` to `MAX_CONTENT_CHARS`, on a character boundary.
fn with_short_content(mut result: SearchResult) -> SearchResult {
    if let Some((end, _)) = result.content.char_indices().nth(MAX_CONTENT_CHARS) {
        result.content.truncate(end);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A provider that fails each request with HTTP 429 and `message`.
    struct Exhausted {
        name: &'static str,
        message: &'static str,
    }

    impl Exhausted {
        fn error(&self) -> ProviderError {
            ProviderError::Status {
                status: 429,
                message: self.message.to_string(),
            }
        }
    }

    impl WebProvider for Exhausted {
        fn name(&self) -> &'static str {
            self.name
        }

        fn search<'a>(
            &'a self,
            _query: &'a str,
            _limit: u32,
        ) -> BoxFuture<'a, Result<Vec<SearchResult>, ProviderError>> {
            Box::pin(async { Err(self.error()) })
        }

        fn fetch<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<String, ProviderError>> {
            Box::pin(async { Err(self.error()) })
        }
    }

    /// A provider that gives one result with `snippet` as its content, and the page text
    /// "text of URL".
    struct Answering {
        name: &'static str,
        snippet: String,
    }

    impl WebProvider for Answering {
        fn name(&self) -> &'static str {
            self.name
        }

        fn search<'a>(
            &'a self,
            _query: &'a str,
            _limit: u32,
        ) -> BoxFuture<'a, Result<Vec<SearchResult>, ProviderError>> {
            Box::pin(async move { Ok(vec![result_with(&self.snippet)]) })
        }

        fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, ProviderError>> {
            Box::pin(async move { Ok(format!("text of {url}")) })
        }
    }

    /// A provider that counts the requests that it gets, and answers each search with no results.
    struct Counting {
        requests: Arc<AtomicUsize>,
    }

    impl WebProvider for Counting {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn search<'a>(
            &'a self,
            _query: &'a str,
            _limit: u32,
        ) -> BoxFuture<'a, Result<Vec<SearchResult>, ProviderError>> {
            Box::pin(async move {
                self.requests.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            })
        }

        fn fetch<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<String, ProviderError>> {
            Box::pin(async move {
                self.requests.fetch_add(1, Ordering::SeqCst);
                Ok(String::new())
            })
        }
    }

    fn result_with(content: &str) -> SearchResult {
        SearchResult {
            title: "Nuclear".to_string(),
            url: "https://nuclearplayer.com/".to_string(),
            content: content.to_string(),
        }
    }

    fn exhausted(name: &'static str, message: &'static str) -> Box<dyn WebProvider> {
        Box::new(Exhausted { name, message })
    }

    fn answering(name: &'static str) -> Box<dyn WebProvider> {
        Box::new(Answering {
            name,
            snippet: "A free music player.".to_string(),
        })
    }

    #[tokio::test]
    async fn search_returns_the_results_of_the_second_provider_when_the_first_fails() {
        let chain = WebChain::new(vec![exhausted("tavily", "limit"), answering("firecrawl")]);

        let results = chain.search("nuclear", 5).await.unwrap();

        assert_eq!(results, vec![result_with("A free music player.")]);
    }

    #[tokio::test]
    async fn search_cuts_the_content_of_each_result_to_500_characters() {
        let chain = WebChain::new(vec![Box::new(Answering {
            name: "exa",
            snippet: "é".repeat(600),
        })]);

        let results = chain.search("nuclear", 5).await.unwrap();

        assert_eq!(results, vec![result_with(&"é".repeat(500))]);
    }

    #[tokio::test]
    async fn fetch_returns_the_text_of_the_second_provider_when_the_first_fails() {
        let chain = WebChain::new(vec![exhausted("tavily", "limit"), answering("firecrawl")]);

        let text = chain.fetch("https://example.com").await.unwrap();

        assert_eq!(text, "text of https://example.com");
    }

    #[tokio::test]
    async fn search_lists_the_error_of_each_provider_in_sequence_when_all_fail() {
        let chain = WebChain::new(vec![
            exhausted("tavily", "Tavily limit"),
            exhausted("firecrawl", "Firecrawl limit"),
            exhausted("exa", "Exa limit"),
            exhausted("parallel", "Parallel limit"),
        ]);

        let error = chain.search("nuclear", 5).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "all web providers failed: tavily: HTTP 429: Tavily limit; firecrawl: HTTP 429: Firecrawl limit; exa: HTTP 429: Exa limit; parallel: HTTP 429: Parallel limit"
        );
    }

    #[tokio::test]
    async fn search_sends_no_request_to_the_second_provider_when_the_first_answers() {
        let requests = Arc::new(AtomicUsize::new(0));
        let chain = WebChain::new(vec![
            answering("tavily"),
            Box::new(Counting {
                requests: Arc::clone(&requests),
            }),
        ]);

        chain.search("nuclear", 5).await.unwrap();

        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }
}
