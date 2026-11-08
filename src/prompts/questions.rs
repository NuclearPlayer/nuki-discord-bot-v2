use std::sync::LazyLock;

use indexmap::IndexMap;
use thiserror::Error;

use crate::prompts::files::DECISION_CATEGORY;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceQuestion {
    pub instructions: String,
    pub options: IndexMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuestionError {
    #[error("the question has no options")]
    NoOptions,
}

/// A Markdown text, divided at each line that starts with `## `.
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

static CATEGORY: LazyLock<ChoiceQuestion> = LazyLock::new(|| {
    parse_choice(DECISION_CATEGORY).expect("category.md is a question with options")
});

pub fn category() -> &'static ChoiceQuestion {
    &CATEGORY
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
    fn category_options_agree_with_the_category_enum() {
        let option_names: Vec<&str> = category().options.keys().map(String::as_str).collect();

        assert_eq!(option_names, Category::ALL.map(Category::name));
    }
}
