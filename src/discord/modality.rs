use std::path::Path;

use rig_core::message::{AudioMediaType, MimeType, VideoMediaType};

use crate::discord::chat_message::AttachmentMeta;

const TEXT_EXTENSIONS: &[&str] = &["log", "txt", "json", "css", "yaml", "yml", "toml", "md"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modality {
    Image,
    Video,
    Audio,
    Text,
    Other,
}

impl Modality {
    pub fn of(attachment: &AttachmentMeta) -> Self {
        let essence = content_type_essence(attachment).unwrap_or_default();
        if essence.starts_with("image/") {
            Self::Image
        } else if essence.starts_with("video/") {
            Self::Video
        } else if essence.starts_with("audio/") {
            Self::Audio
        } else if is_text(essence, &attachment.filename) {
            Self::Text
        } else {
            Self::Other
        }
    }

    /// The name of the modality in the input modalities of a model, for the modalities that a model can lack.
    fn input_name(self) -> Option<&'static str> {
        match self {
            Self::Image => Some("image"),
            Self::Video => Some("video"),
            Self::Audio => Some("audio"),
            Self::Text | Self::Other => None,
        }
    }

    /// The modality as the model receives it: a media modality that the model cannot read is `Other`.
    pub fn for_model(self, input_modalities: &[String]) -> Self {
        match self.input_name() {
            Some(name) if !input_modalities.iter().any(|modality| modality == name) => Self::Other,
            _ => self,
        }
    }
}

pub fn video_media_type(attachment: &AttachmentMeta) -> Option<VideoMediaType> {
    content_type_essence(attachment).and_then(VideoMediaType::from_mime_type)
}

/// Also maps the names that Discord gives MP3 and WAV files, which rig does not know.
pub fn audio_media_type(attachment: &AttachmentMeta) -> Option<AudioMediaType> {
    match content_type_essence(attachment)? {
        "audio/mpeg" => Some(AudioMediaType::MP3),
        "audio/x-wav" | "audio/wave" => Some(AudioMediaType::WAV),
        essence => AudioMediaType::from_mime_type(essence),
    }
}

/// The content type without its parameters: `text/plain; charset=utf-8` is `text/plain`.
fn content_type_essence(attachment: &AttachmentMeta) -> Option<&str> {
    attachment.content_type.as_deref().map(mime_essence)
}

fn mime_essence(content_type: &str) -> &str {
    match content_type.split_once(';') {
        Some((essence, _)) => essence.trim(),
        None => content_type.trim(),
    }
}

fn is_text(essence: &str, filename: &str) -> bool {
    essence.starts_with("text/") || essence == "application/json" || has_text_extension(filename)
}

fn has_text_extension(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            TEXT_EXTENSIONS
                .iter()
                .any(|text_extension| extension.eq_ignore_ascii_case(text_extension))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(filename: &str, content_type: Option<&str>) -> AttachmentMeta {
        AttachmentMeta {
            id: 1400000000000000001,
            filename: filename.to_string(),
            content_type: content_type.map(str::to_string),
            size: 2048,
            url: format!("https://cdn.discordapp.com/attachments/1/2/{filename}"),
        }
    }

    #[test]
    fn modality_of_each_content_type_and_extension() {
        let cases = [
            ("cat.png", Some("image/png"), Modality::Image),
            ("clip.mp4", Some("video/mp4"), Modality::Video),
            ("voice-message.ogg", Some("audio/ogg"), Modality::Audio),
            ("notes", Some("text/plain; charset=utf-8"), Modality::Text),
            (
                "data",
                Some("application/json; charset=utf-8"),
                Modality::Text,
            ),
            ("player.log", None, Modality::Text),
            ("notes.txt", None, Modality::Text),
            ("settings.json", None, Modality::Text),
            (
                "theme.css",
                Some("application/octet-stream"),
                Modality::Text,
            ),
            ("config.yaml", None, Modality::Text),
            ("config.yml", None, Modality::Text),
            ("Cargo.toml", None, Modality::Text),
            ("README.md", None, Modality::Text),
            ("archive.zip", Some("application/zip"), Modality::Other),
            ("unknown", None, Modality::Other),
        ];

        let results: Vec<(&str, Modality)> = cases
            .iter()
            .map(|(filename, content_type, _)| {
                (
                    *filename,
                    Modality::of(&attachment(filename, *content_type)),
                )
            })
            .collect();
        let expected: Vec<(&str, Modality)> = cases
            .iter()
            .map(|(filename, _, modality)| (*filename, *modality))
            .collect();
        assert_eq!(results, expected);
    }
}
