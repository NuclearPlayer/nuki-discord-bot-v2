use std::sync::Arc;

use serde::Deserialize;

use crate::web::chain::{ChainError, WebChain};
use crate::web::provider::SearchResult;

const DEFAULT_SEARCH_LIMIT: u32 = 5;
const MAX_SEARCH_LIMIT: u32 = 20;

/// The chain of web providers that the `web.*` operations use.
pub struct WebOps {
    chain: Arc<WebChain>,
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
    pub fn new(chain: Arc<WebChain>) -> Self {
        Self { chain }
    }

    /// Searches the web, with a limit from 1 to 20.
    pub(super) async fn search(
        &self,
        params: SearchParams,
    ) -> Result<Vec<SearchResult>, ChainError> {
        let limit = params.limit.clamp(1, MAX_SEARCH_LIMIT);
        self.chain.search(&params.query, limit).await
    }

    /// Returns the text of the page at the URL of `params`.
    pub(super) async fn fetch(&self, params: FetchParams) -> Result<String, ChainError> {
        self.chain.fetch(&params.url).await
    }
}
