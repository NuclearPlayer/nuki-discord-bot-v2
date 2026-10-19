use serenity::builder::CreateEmbed;

use crate::workflows::intake::report::{BugReport, FeatureReport, Report, ReportBody};

const BUG_COLOR: u32 = 0xE74C3C;
const FEATURE_COLOR: u32 = 0x9B59B6;

/// One embed field: the name, the value, and whether it is inline.
type Field = (String, String, bool);

/// The parts of an embed that depend on the kind of the report.
struct KindParts {
    heading: String,
    summary: String,
    color: u32,
    fields: Vec<Field>,
}

pub fn report_embed(report: &Report) -> CreateEmbed {
    let parts = kind_parts(&report.body);
    let fields = [
        people_fields(report),
        parts.fields,
        missing_fields(&report.missing),
    ]
    .concat();

    CreateEmbed::new()
        .title(format!(
            "{}{}",
            parts.heading,
            incomplete_mark(&report.missing)
        ))
        .description(parts.summary)
        .color(parts.color)
        .fields(fields)
}

fn kind_parts(body: &ReportBody) -> KindParts {
    match body {
        ReportBody::Bug(bug) => bug_parts(bug),
        ReportBody::Feature(feature) => feature_parts(feature),
    }
}

fn bug_parts(bug: &BugReport) -> KindParts {
    let details = [
        field("Version", &bug.version, true),
        field("Platform", &bug.platform, true),
        field("Plugins", &bug.plugins, true),
        field("Sources", &bug.sources, true),
        field("Item", &bug.item, true),
    ];
    let fixed_in = bug.fixed_in.iter().map(|fixed_in| {
        let value = format!("{} ({})", fixed_in.version, fixed_in.commit);
        field("Possibly fixed in", value, false)
    });

    KindParts {
        heading: format!("🐛 Bug: {}", bug.title),
        summary: bug.summary.clone(),
        color: BUG_COLOR,
        fields: details.into_iter().chain(fixed_in).collect(),
    }
}

fn feature_parts(feature: &FeatureReport) -> KindParts {
    let already_exists = feature.already_exists.as_deref().unwrap_or("No");

    KindParts {
        heading: format!("💡 Feature: {}", feature.title),
        summary: feature.summary.clone(),
        color: FEATURE_COLOR,
        fields: vec![
            field("What", &feature.what, true),
            field("Why", &feature.why, true),
            field("Already exists?", already_exists, false),
        ],
    }
}

fn people_fields(report: &Report) -> Vec<Field> {
    vec![
        field("Reporter", format!("<@{}>", report.reporter_id), true),
        field("Thread", format!("<#{}>", report.thread_id), true),
    ]
}

fn missing_fields(missing: &[String]) -> Vec<Field> {
    if missing.is_empty() {
        return vec![];
    }
    vec![field("Missing", missing.join(", "), false)]
}

fn incomplete_mark(missing: &[String]) -> &'static str {
    if missing.is_empty() {
        return "";
    }
    " · incomplete"
}

fn field(name: &str, value: impl Into<String>, inline: bool) -> Field {
    (name.to_string(), value.into(), inline)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::workflows::intake::report::FixedIn;

    fn bug_report() -> BugReport {
        BugReport {
            title: "Playback stops after one track".to_string(),
            version: "v1.3.0".to_string(),
            platform: "Linux, AppImage".to_string(),
            plugins: "none".to_string(),
            sources: "YouTube".to_string(),
            item: "the album Kid A".to_string(),
            summary: "Playback stops after the first track.".to_string(),
            fixed_in: None,
        }
    }

    fn feature_report(already_exists: Option<String>) -> FeatureReport {
        FeatureReport {
            title: "Crossfade".to_string(),
            what: "Crossfade between tracks".to_string(),
            why: "Gaps between tracks are distracting".to_string(),
            summary: "The reporter wants a crossfade between tracks.".to_string(),
            already_exists,
        }
    }

    fn report(body: ReportBody, missing: Vec<String>) -> Report {
        Report {
            body,
            reporter_id: 111,
            thread_id: 222,
            missing,
        }
    }

    fn embed_json(report: &Report) -> Value {
        serde_json::to_value(report_embed(report)).unwrap()
    }

    #[test]
    fn bug_report_embed_with_all_parts() {
        let bug = BugReport {
            fixed_in: Some(FixedIn {
                version: "v1.4.0".to_string(),
                commit: "abc1234".to_string(),
            }),
            ..bug_report()
        };
        let missing = vec!["Plugins".to_string(), "Sources".to_string()];

        let embed = embed_json(&report(ReportBody::Bug(bug), missing));

        assert_eq!(
            embed,
            json!({
                "type": "rich",
                "title": "🐛 Bug: Playback stops after one track · incomplete",
                "description": "Playback stops after the first track.",
                "color": 0xE74C3C,
                "fields": [
                    { "name": "Reporter", "value": "<@111>", "inline": true },
                    { "name": "Thread", "value": "<#222>", "inline": true },
                    { "name": "Version", "value": "v1.3.0", "inline": true },
                    { "name": "Platform", "value": "Linux, AppImage", "inline": true },
                    { "name": "Plugins", "value": "none", "inline": true },
                    { "name": "Sources", "value": "YouTube", "inline": true },
                    { "name": "Item", "value": "the album Kid A", "inline": true },
                    { "name": "Possibly fixed in", "value": "v1.4.0 (abc1234)", "inline": false },
                    { "name": "Missing", "value": "Plugins, Sources", "inline": false },
                ],
            })
        );
    }

    #[test]
    fn complete_bug_report_embed_has_no_missing_field_and_no_incomplete_mark() {
        let embed = embed_json(&report(ReportBody::Bug(bug_report()), vec![]));

        assert_eq!(
            embed,
            json!({
                "type": "rich",
                "title": "🐛 Bug: Playback stops after one track",
                "description": "Playback stops after the first track.",
                "color": 0xE74C3C,
                "fields": [
                    { "name": "Reporter", "value": "<@111>", "inline": true },
                    { "name": "Thread", "value": "<#222>", "inline": true },
                    { "name": "Version", "value": "v1.3.0", "inline": true },
                    { "name": "Platform", "value": "Linux, AppImage", "inline": true },
                    { "name": "Plugins", "value": "none", "inline": true },
                    { "name": "Sources", "value": "YouTube", "inline": true },
                    { "name": "Item", "value": "the album Kid A", "inline": true },
                ],
            })
        );
    }

    #[test]
    fn feature_report_embed_with_an_existing_feature() {
        let feature = feature_report(Some("The plugin nuclear-crossfade".to_string()));
        let missing = vec!["Why".to_string()];

        let embed = embed_json(&report(ReportBody::Feature(feature), missing));

        assert_eq!(
            embed,
            json!({
                "type": "rich",
                "title": "💡 Feature: Crossfade · incomplete",
                "description": "The reporter wants a crossfade between tracks.",
                "color": 0x9B59B6,
                "fields": [
                    { "name": "Reporter", "value": "<@111>", "inline": true },
                    { "name": "Thread", "value": "<#222>", "inline": true },
                    { "name": "What", "value": "Crossfade between tracks", "inline": true },
                    { "name": "Why", "value": "Gaps between tracks are distracting", "inline": true },
                    { "name": "Already exists?", "value": "The plugin nuclear-crossfade", "inline": false },
                    { "name": "Missing", "value": "Why", "inline": false },
                ],
            })
        );
    }

    #[test]
    fn feature_report_embed_without_an_existing_feature() {
        let embed = embed_json(&report(ReportBody::Feature(feature_report(None)), vec![]));

        assert_eq!(
            embed,
            json!({
                "type": "rich",
                "title": "💡 Feature: Crossfade",
                "description": "The reporter wants a crossfade between tracks.",
                "color": 0x9B59B6,
                "fields": [
                    { "name": "Reporter", "value": "<@111>", "inline": true },
                    { "name": "Thread", "value": "<#222>", "inline": true },
                    { "name": "What", "value": "Crossfade between tracks", "inline": true },
                    { "name": "Why", "value": "Gaps between tracks are distracting", "inline": true },
                    { "name": "Already exists?", "value": "No", "inline": false },
                ],
            })
        );
    }
}
