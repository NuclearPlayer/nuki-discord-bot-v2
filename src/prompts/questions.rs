use std::sync::LazyLock;

use indexmap::IndexMap;
use thiserror::Error;

use crate::prompts::files::{
    DECISION_BUG_FIELDS, DECISION_CATEGORY, DECISION_FEATURE_FIELDS, DECISION_HELP_QUALITY,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceQuestion {
    pub instructions: String,
    pub options: IndexMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuestionError {
    #[error("the question has no options")]
    NoOptions,
    #[error("the question list has text before the first heading")]
    TextBeforeFirstHeading,
}

/// A Markdown text cut at each line that starts with `## `.
struct Sections {
    preamble: String,
    headed: IndexMap<String, String>,
}

impl Sections {
    fn parse(markdown: &str) -> Self {
        // The leading newline lets a heading on the first line match the separator too.
        let text = format!("\n{markdown}");
        let mut parts = text.split("\n## ");
        let preamble = parts
            .next()
            .expect("split yields at least one part")
            .trim()
            .to_string();
        let headed = parts.map(heading_and_body).collect();
        Sections { preamble, headed }
    }
}

fn heading_and_body(section: &str) -> (String, String) {
    let (heading, body) = section.split_once('\n').unwrap_or((section, ""));
    (heading.trim().to_string(), body.trim().to_string())
}

pub fn parse_choice(markdown: &str) -> Result<ChoiceQuestion, QuestionError> {
    let sections = Sections::parse(markdown);
    if sections.headed.is_empty() {
        return Err(QuestionError::NoOptions);
    }
    Ok(ChoiceQuestion {
        instructions: sections.preamble,
        options: sections.headed,
    })
}

pub fn parse_noul_list(markdown: &str) -> Result<IndexMap<String, String>, QuestionError> {
    let sections = Sections::parse(markdown);
    if !sections.preamble.is_empty() {
        return Err(QuestionError::TextBeforeFirstHeading);
    }
    Ok(sections.headed)
}

pub fn parse_noul(markdown: &str) -> String {
    markdown.trim().to_string()
}

static CATEGORY: LazyLock<ChoiceQuestion> = LazyLock::new(|| {
    parse_choice(DECISION_CATEGORY).expect("category.md is a question with options")
});

static HELP_QUALITY: LazyLock<String> = LazyLock::new(|| parse_noul(DECISION_HELP_QUALITY));

static BUG_FIELDS: LazyLock<IndexMap<String, String>> = LazyLock::new(|| {
    parse_noul_list(DECISION_BUG_FIELDS).expect("bug-fields.md is a list of questions")
});

static FEATURE_FIELDS: LazyLock<IndexMap<String, String>> = LazyLock::new(|| {
    parse_noul_list(DECISION_FEATURE_FIELDS).expect("feature-fields.md is a list of questions")
});

pub fn category() -> &'static ChoiceQuestion {
    &CATEGORY
}

pub fn help_quality() -> &'static str {
    &HELP_QUALITY
}

pub fn bug_fields() -> &'static IndexMap<String, String> {
    &BUG_FIELDS
}

pub fn feature_fields() -> &'static IndexMap<String, String> {
    &FEATURE_FIELDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::category::Category;

    fn owned_map<const N: usize>(pairs: [(&str, &str); N]) -> IndexMap<String, String> {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn parse_choice_reads_the_instructions_and_the_options() {
        let markdown = "\nWhich fruit does the author want?\n\n\
            ## apple\n\
            \n\
            A red or green fruit.\n\
            \n\
            It grows on trees.\n\
            \n\
            ## banana\n\
            A long yellow fruit.  \n";

        let question = parse_choice(markdown);

        assert_eq!(
            question,
            Ok(ChoiceQuestion {
                instructions: "Which fruit does the author want?".to_string(),
                options: owned_map([
                    ("apple", "A red or green fruit.\n\nIt grows on trees."),
                    ("banana", "A long yellow fruit."),
                ]),
            })
        );
    }

    #[test]
    fn parse_choice_fails_without_options() {
        let question = parse_choice("Which fruit does the author want?\n\nNo headings here.\n");

        assert_eq!(question, Err(QuestionError::NoOptions));
    }

    #[test]
    fn parse_noul_list_reads_each_question() {
        let markdown = "## colour\n\
            \n\
            Does the conversation state the colour?\n\
            \n\
            ## size\n\
            Does the conversation state the size?\n\
            \n";

        let questions = parse_noul_list(markdown);

        assert_eq!(
            questions,
            Ok(owned_map([
                ("colour", "Does the conversation state the colour?"),
                ("size", "Does the conversation state the size?"),
            ]))
        );
    }

    #[test]
    fn parse_noul_list_fails_with_text_before_the_first_heading() {
        let markdown =
            "Shared instructions.\n\n## colour\n\nDoes the conversation state the colour?\n";

        let questions = parse_noul_list(markdown);

        assert_eq!(questions, Err(QuestionError::TextBeforeFirstHeading));
    }

    #[test]
    fn category_options_agree_with_the_category_enum() {
        let option_names: Vec<&str> = category().options.keys().map(String::as_str).collect();

        assert_eq!(option_names, Category::ALL.map(Category::name));
    }

    #[test]
    fn all_question_files_parse() {
        let bug_field_names: Vec<&str> = bug_fields().keys().map(String::as_str).collect();
        let feature_field_names: Vec<&str> = feature_fields().keys().map(String::as_str).collect();

        assert_eq!(help_quality(), DECISION_HELP_QUALITY.trim());
        assert_eq!(
            bug_field_names,
            ["version", "platform", "plugins", "sources", "item"]
        );
        assert_eq!(feature_field_names, ["what", "why"]);
    }
}
