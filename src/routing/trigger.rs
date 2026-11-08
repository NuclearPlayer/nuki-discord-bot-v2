use serde::Serialize;

/// The reason why a message goes to Nuki.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Direct,
    Unsolicited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageFacts {
    pub author_is_bot: bool,
    pub is_system: bool,
    pub is_dm: bool,
    /// A mention of Nuki's user.
    pub mentions_nuki: bool,
    /// A mention of Nuki's managed role.
    pub mentions_nuki_role: bool,
    pub replies_to_nuki: bool,
}

pub fn trigger_of(facts: &MessageFacts) -> Option<Trigger> {
    if facts.author_is_bot || facts.is_system || facts.is_dm {
        return None;
    }
    if addresses_nuki(facts) {
        return Some(Trigger::Direct);
    }
    Some(Trigger::Unsolicited)
}

/// Returns `true` if the message mentions Nuki, mentions Nuki's managed role, or replies to Nuki.
fn addresses_nuki(facts: &MessageFacts) -> bool {
    facts.mentions_nuki || facts.mentions_nuki_role || facts.replies_to_nuki
}

pub fn trigger_of_unit(unit: &[MessageFacts]) -> Option<Trigger> {
    let triggers: Vec<Trigger> = unit.iter().filter_map(trigger_of).collect();
    if triggers.is_empty() {
        return None;
    }
    if triggers.contains(&Trigger::Direct) {
        return Some(Trigger::Direct);
    }
    Some(Trigger::Unsolicited)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNSOLICITED: MessageFacts = MessageFacts {
        author_is_bot: false,
        is_system: false,
        is_dm: false,
        mentions_nuki: false,
        mentions_nuki_role: false,
        replies_to_nuki: false,
    };

    const MENTION: MessageFacts = MessageFacts {
        mentions_nuki: true,
        ..UNSOLICITED
    };

    const FROM_A_BOT: MessageFacts = MessageFacts {
        author_is_bot: true,
        ..MENTION
    };

    #[test]
    fn messages_from_bots_have_no_trigger() {
        assert_eq!(trigger_of(&FROM_A_BOT), None);
    }

    #[test]
    fn system_messages_have_no_trigger() {
        let facts = MessageFacts {
            is_system: true,
            ..MENTION
        };

        assert_eq!(trigger_of(&facts), None);
    }

    #[test]
    fn dms_have_no_trigger() {
        let facts = MessageFacts {
            is_dm: true,
            ..MENTION
        };

        assert_eq!(trigger_of(&facts), None);
    }

    #[test]
    fn mentions_of_nuki_are_direct() {
        assert_eq!(trigger_of(&MENTION), Some(Trigger::Direct));
    }

    #[test]
    fn mentions_of_the_role_of_nuki_are_direct() {
        let facts = MessageFacts {
            mentions_nuki_role: true,
            ..UNSOLICITED
        };

        assert_eq!(trigger_of(&facts), Some(Trigger::Direct));
    }

    #[test]
    fn replies_to_nuki_are_direct() {
        let facts = MessageFacts {
            replies_to_nuki: true,
            ..UNSOLICITED
        };

        assert_eq!(trigger_of(&facts), Some(Trigger::Direct));
    }

    #[test]
    fn other_messages_are_unsolicited() {
        assert_eq!(trigger_of(&UNSOLICITED), Some(Trigger::Unsolicited));
    }

    #[test]
    fn a_unit_of_messages_without_a_trigger_has_no_trigger() {
        assert_eq!(trigger_of_unit(&[FROM_A_BOT, FROM_A_BOT]), None);
    }

    #[test]
    fn a_unit_with_a_direct_message_is_direct() {
        let unit = [UNSOLICITED, MENTION, FROM_A_BOT];

        assert_eq!(trigger_of_unit(&unit), Some(Trigger::Direct));
    }

    #[test]
    fn a_unit_of_unsolicited_messages_is_unsolicited() {
        let unit = [UNSOLICITED, FROM_A_BOT, UNSOLICITED];

        assert_eq!(trigger_of_unit(&unit), Some(Trigger::Unsolicited));
    }
}
