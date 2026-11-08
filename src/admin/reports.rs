use serde::{Deserialize, Serialize};
use serenity::builder::CreateEmbed;

/// The kind of a report: a bug or a feature request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportKind {
    Bug,
    Feature,
}

/// A bug report or a feature request that Nuki sends to the admin log channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub kind: ReportKind,
    pub title: String,
    pub summary: String,
    /// Facts such as the version, the platform, a fix in a newer version, or a feature that
    /// already exists.
    pub details: Option<String>,
    /// The author of the message that the report is about.
    pub reporter_id: u64,
    /// The thread where the conversation about the report continues.
    pub thread_id: u64,
}

const BUG_COLOR: u32 = 0xE74C3C;
const FEATURE_COLOR: u32 = 0x9B59B6;

/// One embed field: the name, the value, and the inline flag.
type Field = (String, String, bool);

/// Makes the embed of the report for the admin log channel. Adds the `Details` field only if the
/// report has details.
pub fn report_embed(report: &Report) -> CreateEmbed {
    let (heading, color) = match report.kind {
        ReportKind::Bug => ("🐛 Bug", BUG_COLOR),
        ReportKind::Feature => ("💡 Feature", FEATURE_COLOR),
    };
    let people = [
        field("Reporter", format!("<@{}>", report.reporter_id), true),
        field("Thread", format!("<#{}>", report.thread_id), true),
    ];
    let details = report
        .details
        .iter()
        .map(|details| field("Details", details.clone(), false));

    CreateEmbed::new()
        .title(format!("{heading}: {}", report.title))
        .description(report.summary.clone())
        .color(color)
        .fields(people.into_iter().chain(details))
}

fn field(name: &str, value: String, inline: bool) -> Field {
    (name.to_string(), value, inline)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn report(kind: ReportKind, title: &str, details: Option<&str>) -> Report {
        Report {
            kind,
            title: title.to_string(),
            summary: "Playback stops after the first track of an album.".to_string(),
            details: details.map(str::to_string),
            reporter_id: 111,
            thread_id: 222,
        }
    }

    fn embed_json(report: &Report) -> Value {
        serde_json::to_value(report_embed(report)).unwrap()
    }

    #[test]
    fn bug_report_embed_with_details() {
        let bug = report(
            ReportKind::Bug,
            "Playback stops after one track",
            Some("Version 1.3.0 on Linux, AppImage. No fix in 1.4.0."),
        );

        assert_eq!(
            embed_json(&bug),
            json!({
                "type": "rich",
                "title": "🐛 Bug: Playback stops after one track",
                "description": "Playback stops after the first track of an album.",
                "color": 0xE74C3C,
                "fields": [
                    { "name": "Reporter", "value": "<@111>", "inline": true },
                    { "name": "Thread", "value": "<#222>", "inline": true },
                    {
                        "name": "Details",
                        "value": "Version 1.3.0 on Linux, AppImage. No fix in 1.4.0.",
                        "inline": false,
                    },
                ],
            })
        );
    }

    #[test]
    fn feature_report_embed_without_details() {
        let feature = Report {
            summary: "The reporter wants a crossfade between tracks.".to_string(),
            ..report(ReportKind::Feature, "Crossfade", None)
        };

        assert_eq!(
            embed_json(&feature),
            json!({
                "type": "rich",
                "title": "💡 Feature: Crossfade",
                "description": "The reporter wants a crossfade between tracks.",
                "color": 0x9B59B6,
                "fields": [
                    { "name": "Reporter", "value": "<@111>", "inline": true },
                    { "name": "Thread", "value": "<#222>", "inline": true },
                ],
            })
        );
    }
}
