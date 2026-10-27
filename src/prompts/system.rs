use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};

use crate::prompts::files::{
    KNOWLEDGE, NUCLEAR, PERSONALITY, WORKFLOW_BUG_INTAKE, WORKFLOW_CHAT, WORKFLOW_FEATURE_INTAKE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowPrompt {
    BugIntake,
    FeatureIntake,
    Chat,
}

impl WorkflowPrompt {
    pub fn text(self) -> &'static str {
        match self {
            WorkflowPrompt::BugIntake => WORKFLOW_BUG_INTAKE,
            WorkflowPrompt::FeatureIntake => WORKFLOW_FEATURE_INTAKE,
            WorkflowPrompt::Chat => WORKFLOW_CHAT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEmoji {
    pub name: String,
    pub id: u64,
    pub animated: bool,
}

impl fmt::Display for CustomEmoji {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.animated {
            write!(f, "<a:{}:{}>", self.name, self.id)
        } else {
            write!(f, "<:{}:{}>", self.name, self.id)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeContext {
    pub bot_name: String,
    pub bot_user_id: u64,
    pub emoji: Vec<CustomEmoji>,
    pub channel_name: String,
    pub now: DateTime<Utc>,
}

impl RuntimeContext {
    fn section(&self) -> String {
        format!(
            "## Current context\n\
            \n\
            You are {name}, with the user ID {id}. Users mention you with <@{id}>.\n\
            Custom emoji of this server: {emoji}\n\
            Current channel: #{channel}\n\
            Current time: {now}",
            name = self.bot_name,
            id = self.bot_user_id,
            emoji = emoji_list(&self.emoji),
            channel = self.channel_name,
            now = self.now.to_rfc3339_opts(SecondsFormat::Secs, true),
        )
    }
}

fn emoji_list(emoji: &[CustomEmoji]) -> String {
    if emoji.is_empty() {
        return "none".to_string();
    }
    emoji
        .iter()
        .map(CustomEmoji::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

pub struct SystemPrompt;

impl SystemPrompt {
    pub fn build(
        workflow: WorkflowPrompt,
        extra: Option<&str>,
        runtime: &RuntimeContext,
    ) -> String {
        let runtime_section = runtime.section();
        [PERSONALITY, NUCLEAR, KNOWLEDGE, workflow.text()]
            .into_iter()
            .chain(extra)
            .chain([runtime_section.as_str()])
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_with_emoji(emoji: Vec<CustomEmoji>) -> RuntimeContext {
        RuntimeContext {
            bot_name: "Nuki".to_string(),
            bot_user_id: 1_234_567_890,
            emoji,
            channel_name: "general".to_string(),
            now: DateTime::parse_from_rfc3339("2026-10-19T12:34:56.789Z")
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    fn two_emoji() -> Vec<CustomEmoji> {
        vec![
            CustomEmoji {
                name: "nuki_wave".to_string(),
                id: 111,
                animated: false,
            },
            CustomEmoji {
                name: "nuki_dance".to_string(),
                id: 222,
                animated: true,
            },
        ]
    }

    #[test]
    fn section_lists_the_custom_emoji_of_the_server() {
        let runtime = runtime_with_emoji(two_emoji());

        assert_eq!(
            runtime.section(),
            "## Current context\n\
            \n\
            You are Nuki, with the user ID 1234567890. Users mention you with <@1234567890>.\n\
            Custom emoji of this server: <:nuki_wave:111>, <a:nuki_dance:222>\n\
            Current channel: #general\n\
            Current time: 2026-10-19T12:34:56Z"
        );
    }

    #[test]
    fn section_writes_none_when_the_server_has_no_custom_emoji() {
        let runtime = runtime_with_emoji(Vec::new());

        assert_eq!(
            runtime.section(),
            "## Current context\n\
            \n\
            You are Nuki, with the user ID 1234567890. Users mention you with <@1234567890>.\n\
            Custom emoji of this server: none\n\
            Current channel: #general\n\
            Current time: 2026-10-19T12:34:56Z"
        );
    }
}
