use std::sync::Arc;

use serde::Deserialize;

use crate::web::client::{TavilyClient, TavilyError};
use crate::web::types::SearchResult;

const DEFAULT_SEARCH_LIMIT: u32 = 5;
const MAX_SEARCH_LIMIT: u32 = 20;

/// The Tavily client that the `web.*` operations use.
pub struct WebOps {
    tavily: Arc<TavilyClient>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchParams {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FetchParams {
    url: String,
}

fn default_search_limit() -> u32 {
    DEFAULT_SEARCH_LIMIT
}

impl WebOps {
    pub fn new(tavily: Arc<TavilyClient>) -> Self {
        Self { tavily }
    }

    /// Searches the web, with a limit from 1 to 20.
    pub(super) async fn search(
        &self,
        params: SearchParams,
    ) -> Result<Vec<SearchResult>, TavilyError> {
        let limit = params.limit.clamp(1, MAX_SEARCH_LIMIT);
        self.tavily.search(&params.query, limit).await
    }

    /// Returns the text of the page at the URL of `params`.
    pub(super) async fn fetch(&self, params: FetchParams) -> Result<String, TavilyError> {
        self.tavily.extract(&params.url).await
    }
}
