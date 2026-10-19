/// Why a message reaches Nuki.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    IntakeThread,
    Direct,
    Unsolicited,
}

/// A trigger that goes through the Jev classification. Intake thread messages go directly to their intake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifiedTrigger {
    Direct,
    Unsolicited,
}

impl From<ClassifiedTrigger> for Trigger {
    fn from(trigger: ClassifiedTrigger) -> Trigger {
        match trigger {
            ClassifiedTrigger::Direct => Trigger::Direct,
            ClassifiedTrigger::Unsolicited => Trigger::Unsolicited,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageFacts {
    pub author_is_bot: bool,
    pub is_system: bool,
    pub is_dm: bool,
    pub in_open_intake_thread: bool,
    pub mentions_nuki: bool,
    pub replies_to_nuki: bool,
}

pub fn trigger_of(facts: &MessageFacts) -> Option<Trigger> {
    if facts.author_is_bot || facts.is_system || facts.is_dm {
        return None;
    }
    if facts.in_open_intake_thread {
        return Some(Trigger::IntakeThread);
    }
    if facts.mentions_nuki || facts.replies_to_nuki {
        return Some(Trigger::Direct);
    }
    Some(Trigger::Unsolicited)
}

pub fn trigger_of_unit(unit: &[MessageFacts]) -> Option<Trigger> {
    let triggers: Vec<Trigger> = unit.iter().filter_map(trigger_of).collect();
    if triggers.is_empty() {
        return None;
    }
    if triggers.contains(&Trigger::IntakeThread) {
        return Some(Trigger::IntakeThread);
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
        in_open_intake_thread: false,
        mentions_nuki: false,
        replies_to_nuki: false,
    };

    const MENTION: MessageFacts = MessageFacts {
        mentions_nuki: true,
        ..UNSOLICITED
    };

    const IN_INTAKE_THREAD: MessageFacts = MessageFacts {
        in_open_intake_thread: true,
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
    fn messages_in_an_open_intake_thread_are_intake_thread() {
        assert_eq!(trigger_of(&IN_INTAKE_THREAD), Some(Trigger::IntakeThread));
    }

    #[test]
    fn mentions_of_nuki_are_direct() {
        assert_eq!(trigger_of(&MENTION), Some(Trigger::Direct));
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
    fn intake_thread_wins_over_a_mention() {
        let facts = MessageFacts {
            mentions_nuki: true,
            ..IN_INTAKE_THREAD
        };

        assert_eq!(trigger_of(&facts), Some(Trigger::IntakeThread));
    }

    #[test]
    fn a_unit_of_messages_without_a_trigger_has_no_trigger() {
        assert_eq!(trigger_of_unit(&[FROM_A_BOT, FROM_A_BOT]), None);
    }

    #[test]
    fn a_unit_with_an_intake_thread_message_is_intake_thread() {
        let unit = [UNSOLICITED, IN_INTAKE_THREAD, MENTION];

        assert_eq!(trigger_of_unit(&unit), Some(Trigger::IntakeThread));
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
