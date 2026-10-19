use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BugReport {
    #[schemars(description = "A short title of the bug, at most 80 characters.")]
    pub title: String,
    #[schemars(description = "The Nuclear version of the reporter, or \"unknown\".")]
    pub version: String,
    #[schemars(
        description = "The operating system of the reporter, and the package format if the reporter knows it, or \"unknown\"."
    )]
    pub platform: String,
    #[schemars(description = "The plugins that the reporter uses, or \"unknown\".")]
    pub plugins: String,
    #[schemars(
        description = "The sources that the reporter uses: metadata providers and streaming providers, or \"unknown\"."
    )]
    pub sources: String,
    #[schemars(
        description = "The album, the artist, or the track that has the problem, or \"unknown\"."
    )]
    pub item: String,
    #[schemars(description = "A summary of the bug in two to four sentences.")]
    pub summary: String,
    #[schemars(
        description = "The newer version that possibly fixed the bug, or null if the check found no fix."
    )]
    pub fixed_in: Option<FixedIn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FixedIn {
    #[schemars(description = "The first version that contains the fix, for example \"v1.4.0\".")]
    pub version: String,
    #[schemars(description = "The hash of the commit that possibly fixed the bug.")]
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeatureReport {
    #[schemars(description = "A short title of the feature, at most 80 characters.")]
    pub title: String,
    #[schemars(description = "What the reporter wants, or \"unknown\".")]
    pub what: String,
    #[schemars(description = "Why the reporter wants it, or \"unknown\".")]
    pub why: String,
    #[schemars(description = "A summary of the feature request in two to four sentences.")]
    pub summary: String,
    #[schemars(
        description = "Where the feature already exists: in Nuclear, in a plugin, or in an open pull request. Null if it does not exist."
    )]
    pub already_exists: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReportBody {
    Bug(BugReport),
    Feature(FeatureReport),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub body: ReportBody,
    pub reporter_id: u64,
    pub thread_id: u64,
    /// The names of the checklist fields that the conversation did not give.
    pub missing: Vec<String>,
}
