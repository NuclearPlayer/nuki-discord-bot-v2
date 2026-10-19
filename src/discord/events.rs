//! The usage events of the message pipeline, built from the messages and the routing.

use serenity::model::channel::{Attachment as DiscordAttachment, Message};

use crate::routing::classify::Classification;
use crate::routing::dispatch::Decision;
use crate::routing::trigger::{MessageFacts, Trigger};
use crate::stats::event::{Attachment, MessageReceived, UnitRouted};

/// The `message.received` event of a message with `facts`, in a unit with `trigger`.
pub(crate) fn message_received(
    message: &Message,
    facts: &MessageFacts,
    trigger: Trigger,
) -> MessageReceived {
    MessageReceived {
        message_id: message.id.get(),
        trigger,
        mentions_nuki: facts.mentions_nuki,
        mentions_nuki_role: facts.mentions_nuki_role,
        replies_to_nuki: facts.replies_to_nuki,
        attachments: message.attachments.iter().map(attachment_of).collect(),
        content: message.content.clone(),
    }
}

fn attachment_of(attachment: &DiscordAttachment) -> Attachment {
    Attachment {
        id: attachment.id.get(),
        filename: attachment.filename.clone(),
        content_type: attachment.content_type.clone(),
        size: u64::from(attachment.size),
    }
}

/// The `unit.routed` event of a unit of `message_count` messages.
pub(crate) fn unit_routed(
    message_count: usize,
    classification: &Classification,
    decision: &Decision,
) -> UnitRouted {
    UnitRouted {
        message_count: message_count as u64,
        probabilities: classification
            .probabilities
            .iter()
            .map(|(category, probability)| (category.name().to_string(), *probability))
            .collect(),
        decision: decision.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use indexmap::indexmap;

    use super::*;
    use crate::routing::category::Category;

    #[test]
    fn message_received_of_a_mention_of_the_role_of_nuki() {
        let message: Message = serde_json::from_str(include_str!(
            "../../tests/fixtures/discord/handler_role_mention.json"
        ))
        .unwrap();
        let facts = MessageFacts {
            author_is_bot: false,
            is_system: false,
            is_dm: false,
            in_open_intake_thread: false,
            mentions_nuki: false,
            mentions_nuki_role: true,
            replies_to_nuki: false,
        };

        assert_eq!(
            message_received(&message, &facts, Trigger::Direct),
            MessageReceived {
                message_id: 1100000000000000017,
                trigger: Trigger::Direct,
                mentions_nuki: false,
                mentions_nuki_role: true,
                replies_to_nuki: false,
                attachments: vec![],
                content: "<@&1900000000000000001> how do I add a plugin?".to_string(),
            }
        );
    }

    #[test]
    fn unit_routed_of_a_run_decision() {
        let classification = Classification {
            probabilities: indexmap! {
                Category::HelpRequest => 0.78,
                Category::Chat => 0.22,
            },
        };

        assert_eq!(
            unit_routed(2, &classification, &Decision::Run(Category::HelpRequest)),
            UnitRouted {
                message_count: 2,
                probabilities: BTreeMap::from([
                    ("help_request".to_string(), 0.78),
                    ("chat".to_string(), 0.22),
                ]),
                decision: "run help_request".to_string(),
            }
        );
    }
}
