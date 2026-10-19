use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Choice {
        instructions: String,
        criteria: IndexMap<String, String>,
    },
    Noul {
        instructions: String,
    },
}

#[derive(Debug, Serialize)]
pub struct DecisionRequest {
    pub model: String,
    pub state: serde_json::Value,
    pub questions: IndexMap<String, Question>,
}

/// Serde ignores the fields that the bot does not use, for example `confidence`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: IndexMap<String, f64>,
    },
    Noul {
        noul: f64,
    },
}

#[derive(Debug, Deserialize)]
pub struct DecisionResponse {
    pub answers: IndexMap<String, Answer>,
    /// The Jev model that answered.
    pub model: Option<String>,
    pub usage: Option<JevUsage>,
}

#[derive(Debug, Deserialize)]
pub struct JevUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// In USD.
    pub cost: Option<f64>,
}
