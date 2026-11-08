use std::fmt;
use std::time::Instant;

use super::category::Category;
use super::classify::Classification;
use super::cooldown::Cooldowns;
use super::trigger::Trigger;
use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub help: f64,
    pub chat: f64,
}

impl From<&Config> for Thresholds {
    fn from(config: &Config) -> Thresholds {
        Thresholds {
            help: config.threshold_help,
            chat: config.threshold_chat,
        }
    }
}

impl Thresholds {
    /// Returns the threshold of a category that has a workflow. `Ignore` has no threshold.
    fn of(&self, category: Category) -> Option<f64> {
        match category {
            Category::Help => Some(self.help),
            Category::Chat => Some(self.chat),
            Category::Ignore => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Run(Category),
    Skip(SkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SkipReason {
    Ignored,
    BelowThreshold {
        category: Category,
        probability: f64,
        threshold: f64,
    },
    Cooldown,
}

pub fn decide(
    trigger: Trigger,
    classification: &Classification,
    thresholds: &Thresholds,
    chat_cooldown_active: bool,
) -> Decision {
    let (category, probability) = classification.top();
    match trigger {
        Trigger::Direct => Decision::Run(category),
        Trigger::Unsolicited => {
            decide_unsolicited(category, probability, thresholds, chat_cooldown_active)
        }
    }
}

/// Decides with the chat cooldown of the channel `channel_id`. If an unsolicited chat runs,
/// starts the cooldown at the decision, not at the reply, because a unit that the bot decides
/// while Nuki writes the reply must see the cooldown.
pub fn decide_with_cooldown(
    trigger: Trigger,
    classification: &Classification,
    thresholds: &Thresholds,
    cooldowns: &Cooldowns,
    channel_id: u64,
    now: Instant,
) -> Decision {
    let cooldown_active = cooldowns.is_active(channel_id, now);
    let decision = decide(trigger, classification, thresholds, cooldown_active);
    if trigger == Trigger::Unsolicited && decision == Decision::Run(Category::Chat) {
        cooldowns.start(channel_id, now);
    }
    decision
}

fn decide_unsolicited(
    category: Category,
    probability: f64,
    thresholds: &Thresholds,
    chat_cooldown_active: bool,
) -> Decision {
    let Some(threshold) = thresholds.of(category) else {
        return Decision::Skip(SkipReason::Ignored);
    };
    if probability < threshold {
        return Decision::Skip(SkipReason::BelowThreshold {
            category,
            probability,
            threshold,
        });
    }
    if category == Category::Chat && chat_cooldown_active {
        return Decision::Skip(SkipReason::Cooldown);
    }
    Decision::Run(category)
}

/// Returns the log line of a classification, for example
/// `help=0.78 chat=0.19 ignore=0.03 -> run help`.
/// For a direct classification, the line has no `ignore` probability, because the
/// classification has none.
pub fn describe(classification: &Classification, decision: &Decision) -> String {
    let probabilities: Vec<String> = Category::ALL
        .into_iter()
        .filter_map(|category| {
            classification
                .probabilities
                .get(&category)
                .map(|probability| format!("{}={probability:.2}", category.name()))
        })
        .collect();
    format!("{} -> {decision}", probabilities.join(" "))
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Decision::Run(category) => write!(f, "run {}", category.name()),
            Decision::Skip(reason) => write!(f, "skip: {reason}"),
        }
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Ignored => write!(f, "ignored"),
            SkipReason::BelowThreshold {
                category,
                probability,
                threshold,
            } => write!(
                f,
                "{} {probability:.2} is below the threshold {threshold:.2}",
                category.name()
            ),
            SkipReason::Cooldown => write!(f, "chat cooldown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use indexmap::indexmap;

    use super::*;

    const THRESHOLDS: Thresholds = Thresholds {
        help: 0.7,
        chat: 0.5,
    };
    const COOLDOWN: Duration = Duration::from_secs(15 * 60);
    const CHANNEL_ID: u64 = 1200000000000000001;

    fn classification_with_top(category: Category, probability: f64) -> Classification {
        let other = Category::ALL
            .into_iter()
            .find(|candidate| *candidate != category)
            .unwrap();
        Classification {
            probabilities: indexmap! {
                other => 0.05,
                category => probability,
            },
        }
    }

    #[test]
    fn direct_runs_the_top_category() {
        let classification = classification_with_top(Category::Help, 0.9);

        let decision = decide(Trigger::Direct, &classification, &THRESHOLDS, false);

        assert_eq!(decision, Decision::Run(Category::Help));
    }

    #[test]
    fn direct_runs_even_below_the_threshold() {
        let classification = classification_with_top(Category::Chat, 0.4);

        let decision = decide(Trigger::Direct, &classification, &THRESHOLDS, false);

        assert_eq!(decision, Decision::Run(Category::Chat));
    }

    #[test]
    fn unsolicited_skips_ignore() {
        let classification = classification_with_top(Category::Ignore, 0.95);

        let decision = decide(Trigger::Unsolicited, &classification, &THRESHOLDS, false);

        assert_eq!(decision, Decision::Skip(SkipReason::Ignored));
    }

    #[test]
    fn unsolicited_skips_below_the_threshold() {
        let classification = classification_with_top(Category::Help, 0.65);

        let decision = decide(Trigger::Unsolicited, &classification, &THRESHOLDS, false);

        assert_eq!(
            decision,
            Decision::Skip(SkipReason::BelowThreshold {
                category: Category::Help,
                probability: 0.65,
                threshold: 0.7,
            })
        );
    }

    #[test]
    fn unsolicited_runs_at_the_threshold() {
        let classification = classification_with_top(Category::Help, 0.7);

        let decision = decide(Trigger::Unsolicited, &classification, &THRESHOLDS, false);

        assert_eq!(decision, Decision::Run(Category::Help));
    }

    #[test]
    fn unsolicited_chat_skips_during_the_cooldown() {
        let classification = classification_with_top(Category::Chat, 0.9);

        let decision = decide(Trigger::Unsolicited, &classification, &THRESHOLDS, true);

        assert_eq!(decision, Decision::Skip(SkipReason::Cooldown));
    }

    #[test]
    fn unsolicited_help_runs_during_the_chat_cooldown() {
        let classification = classification_with_top(Category::Help, 0.9);

        let decision = decide(Trigger::Unsolicited, &classification, &THRESHOLDS, true);

        assert_eq!(decision, Decision::Run(Category::Help));
    }

    #[test]
    fn an_unsolicited_chat_decision_starts_the_cooldown() {
        let cooldowns = Cooldowns::new(COOLDOWN);
        let classification = classification_with_top(Category::Chat, 0.9);
        let now = Instant::now();

        let decisions = [now, now + Duration::from_secs(1)].map(|at| {
            decide_with_cooldown(
                Trigger::Unsolicited,
                &classification,
                &THRESHOLDS,
                &cooldowns,
                CHANNEL_ID,
                at,
            )
        });

        assert_eq!(
            decisions,
            [
                Decision::Run(Category::Chat),
                Decision::Skip(SkipReason::Cooldown)
            ]
        );
    }

    #[test]
    fn a_direct_chat_decision_does_not_start_the_cooldown() {
        let cooldowns = Cooldowns::new(COOLDOWN);
        let classification = classification_with_top(Category::Chat, 0.9);
        let now = Instant::now();

        decide_with_cooldown(
            Trigger::Direct,
            &classification,
            &THRESHOLDS,
            &cooldowns,
            CHANNEL_ID,
            now,
        );

        assert!(!cooldowns.is_active(CHANNEL_ID, now));
    }

    #[test]
    fn describe_writes_all_probabilities_and_the_decision() {
        let classification = Classification {
            probabilities: indexmap! {
                Category::Ignore => 0.03,
                Category::Chat => 0.19,
                Category::Help => 0.78,
            },
        };

        let line = describe(&classification, &Decision::Run(Category::Help));

        assert_eq!(line, "help=0.78 chat=0.19 ignore=0.03 -> run help");
    }

    #[test]
    fn describe_writes_the_skip_reason() {
        let classification = Classification {
            probabilities: indexmap! {
                Category::Help => 0.05,
                Category::Chat => 0.4,
            },
        };
        let decision = Decision::Skip(SkipReason::BelowThreshold {
            category: Category::Chat,
            probability: 0.4,
            threshold: 0.5,
        });

        let line = describe(&classification, &decision);

        assert_eq!(
            line,
            "help=0.05 chat=0.40 -> skip: chat 0.40 is below the threshold 0.50"
        );
    }
}
