use serde::{Deserialize, Serialize};

/// One result of a Tavily search, as Tavily sends it and as the scripts get it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SearchRequest<'a> {
    pub query: &'a str,
    pub max_results: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExtractRequest<'a> {
    pub urls: [&'a str; 1],
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExtractResponse {
    #[serde(default)]
    pub results: Vec<ExtractedPage>,
    #[serde(default)]
    pub failed_results: Vec<FailedExtraction>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExtractedPage {
    pub raw_content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FailedExtraction {
    pub error: String,
}

/// The body of a Tavily error, for example `{ "detail": { "error": "..." } }`.
#[derive(Debug, Deserialize)]
pub(crate) struct ApiErrorBody {
    pub detail: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiErrorDetail {
    pub error: String,
}
