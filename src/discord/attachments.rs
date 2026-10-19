use futures::future::try_join_all;
use reqwest::header::RANGE;
use reqwest::{RequestBuilder, Response};
use rig_core::message::{AudioMediaType, VideoMediaType};

use crate::discord::chat_message::{AttachmentMeta, ChatMessage};
use crate::discord::modality::{Modality, audio_media_type, video_media_type};

const TEXT_LIMIT_BYTES: u64 = 20_000;

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedAttachment {
    Image {
        label: String,
        url: String,
    },
    Video {
        label: String,
        url: String,
        media_type: VideoMediaType,
    },
    Audio {
        label: String,
        data: Vec<u8>,
        media_type: AudioMediaType,
    },
    Text {
        label: String,
        content: String,
    },
    Other {
        label: String,
    },
}

impl ResolvedAttachment {
    pub fn label(&self) -> &str {
        match self {
            Self::Image { label, .. }
            | Self::Video { label, .. }
            | Self::Audio { label, .. }
            | Self::Text { label, .. }
            | Self::Other { label } => label,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMessage {
    pub message: ChatMessage,
    pub attachments: Vec<ResolvedAttachment>,
}

pub async fn resolve(
    attachment: &AttachmentMeta,
    input_modalities: &[String],
    http: &reqwest::Client,
) -> Result<ResolvedAttachment, reqwest::Error> {
    let resolved = match Modality::of(attachment).for_model(input_modalities) {
        Modality::Image => ResolvedAttachment::Image {
            label: label(attachment),
            url: attachment.url.clone(),
        },
        Modality::Video => resolve_video(attachment),
        Modality::Audio => resolve_audio(attachment, http).await?,
        Modality::Text => ResolvedAttachment::Text {
            label: label(attachment),
            content: download_text(attachment, http).await?,
        },
        Modality::Other => other(attachment),
    };
    Ok(resolved)
}

/// A video whose media type rig does not know is `Other`.
fn resolve_video(attachment: &AttachmentMeta) -> ResolvedAttachment {
    match video_media_type(attachment) {
        Some(media_type) => ResolvedAttachment::Video {
            label: label(attachment),
            url: attachment.url.clone(),
            media_type,
        },
        None => other(attachment),
    }
}

/// An audio file whose media type rig does not know is `Other`, and is not downloaded.
async fn resolve_audio(
    attachment: &AttachmentMeta,
    http: &reqwest::Client,
) -> Result<ResolvedAttachment, reqwest::Error> {
    let resolved = match audio_media_type(attachment) {
        Some(media_type) => ResolvedAttachment::Audio {
            label: label(attachment),
            data: fetch(http.get(&attachment.url))
                .await?
                .bytes()
                .await?
                .to_vec(),
            media_type,
        },
        None => other(attachment),
    };
    Ok(resolved)
}

fn other(attachment: &AttachmentMeta) -> ResolvedAttachment {
    ResolvedAttachment::Other {
        label: other_label(attachment),
    }
}

pub async fn resolve_message(
    message: &ChatMessage,
    input_modalities: &[String],
    http: &reqwest::Client,
) -> Result<ResolvedMessage, reqwest::Error> {
    let attachments = try_join_all(
        message
            .attachments
            .iter()
            .map(|attachment| resolve(attachment, input_modalities, http)),
    )
    .await?;
    Ok(ResolvedMessage {
        message: message.clone(),
        attachments,
    })
}

fn label(attachment: &AttachmentMeta) -> String {
    format!("[attachment {}: {}]", attachment.id, attachment.filename)
}

fn other_label(attachment: &AttachmentMeta) -> String {
    let content_type = attachment.content_type.as_deref().unwrap_or("unknown");
    format!(
        "[attachment {}: {}, {} bytes, {content_type}]",
        attachment.id, attachment.filename, attachment.size
    )
}

async fn fetch(request: RequestBuilder) -> Result<Response, reqwest::Error> {
    request.send().await?.error_for_status()
}

async fn download_text(
    attachment: &AttachmentMeta,
    http: &reqwest::Client,
) -> Result<String, reqwest::Error> {
    if attachment.size > TEXT_LIMIT_BYTES {
        download_text_tail(attachment, http).await
    } else {
        fetch(http.get(&attachment.url)).await?.text().await
    }
}

/// Downloads only the last `TEXT_LIMIT_BYTES` bytes, keeps them from the next character boundary, and puts a note of the removed byte count before them.
async fn download_text_tail(
    attachment: &AttachmentMeta,
    http: &reqwest::Client,
) -> Result<String, reqwest::Error> {
    let request = http
        .get(&attachment.url)
        .header(RANGE, format!("bytes=-{TEXT_LIMIT_BYTES}"));
    let tail = fetch(request).await?.bytes().await?;
    let kept = from_character_boundary(&tail);
    let removed = attachment.size.saturating_sub(kept.len() as u64);
    Ok(format!(
        "[first {removed} bytes removed]\n{}",
        String::from_utf8_lossy(kept)
    ))
}

/// Skips the UTF-8 continuation bytes at the start, which belong to a character that the range cut.
fn from_character_boundary(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| byte & 0b1100_0000 != 0b1000_0000)
        .unwrap_or(bytes.len());
    &bytes[start..]
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::discord::test_support::attachment;

    fn modalities(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    async fn server_serving(file_path: &str, response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(file_path))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn an_image_is_a_url_when_the_model_reads_images() {
        let image = attachment("cat.png", Some("image/png"));

        let resolved = resolve(
            &image,
            &modalities(&["text", "image"]),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Image {
                label: "[attachment 1400000000000000001: cat.png]".to_string(),
                url: "https://cdn.discordapp.com/attachments/1/2/cat.png".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn a_video_has_its_url_and_media_type() {
        let video = attachment("clip.mp4", Some("video/mp4"));

        let resolved = resolve(
            &video,
            &modalities(&["text", "video"]),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Video {
                label: "[attachment 1400000000000000001: clip.mp4]".to_string(),
                url: "https://cdn.discordapp.com/attachments/1/2/clip.mp4".to_string(),
                media_type: VideoMediaType::MP4,
            }
        );
    }

    #[tokio::test]
    async fn an_image_is_other_when_the_model_does_not_read_images() {
        let image = attachment("cat.png", Some("image/png"));

        let resolved = resolve(&image, &modalities(&["text"]), &reqwest::Client::new())
            .await
            .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Other {
                label: "[attachment 1400000000000000001: cat.png, 2048 bytes, image/png]"
                    .to_string(),
            }
        );
    }

    #[tokio::test]
    async fn audio_is_downloaded() {
        let bytes = vec![0x4f, 0x67, 0x67, 0x53, 0x00, 0x02];
        let server = server_serving(
            "/voice-message.ogg",
            ResponseTemplate::new(200).set_body_bytes(bytes.clone()),
        )
        .await;
        let audio = AttachmentMeta {
            url: format!("{}/voice-message.ogg", server.uri()),
            ..attachment("voice-message.ogg", Some("audio/ogg"))
        };

        let resolved = resolve(
            &audio,
            &modalities(&["text", "audio"]),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Audio {
                label: "[attachment 1400000000000000001: voice-message.ogg]".to_string(),
                data: bytes,
                media_type: AudioMediaType::OGG,
            }
        );
    }

    #[tokio::test]
    async fn audio_mpeg_is_sent_as_mp3() {
        let bytes = vec![0x49, 0x44, 0x33, 0x04];
        let server = server_serving(
            "/song.mp3",
            ResponseTemplate::new(200).set_body_bytes(bytes.clone()),
        )
        .await;
        let audio = AttachmentMeta {
            url: format!("{}/song.mp3", server.uri()),
            ..attachment("song.mp3", Some("audio/mpeg"))
        };

        let resolved = resolve(
            &audio,
            &modalities(&["text", "audio"]),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Audio {
                label: "[attachment 1400000000000000001: song.mp3]".to_string(),
                data: bytes,
                media_type: AudioMediaType::MP3,
            }
        );
    }

    #[tokio::test]
    async fn text_is_inlined() {
        let server = server_serving(
            "/player.log",
            ResponseTemplate::new(200)
                .set_body_raw("started\ncrashed\n", "text/plain; charset=utf-8"),
        )
        .await;
        let log = AttachmentMeta {
            url: format!("{}/player.log", server.uri()),
            ..attachment("player.log", Some("text/plain; charset=utf-8"))
        };

        let resolved = resolve(&log, &modalities(&["text"]), &reqwest::Client::new())
            .await
            .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Text {
                label: "[attachment 1400000000000000001: player.log]".to_string(),
                content: "started\ncrashed\n".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn long_text_keeps_the_last_20_kb_from_a_character_boundary() {
        let tail = "b".repeat(19_999);
        let file = format!("aé{tail}");
        let last_20_kb = file.as_bytes()[file.len() - 20_000..].to_vec();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/player.log"))
            .and(header("range", "bytes=-20000"))
            .respond_with(ResponseTemplate::new(206).set_body_raw(last_20_kb, "text/plain"))
            .mount(&server)
            .await;
        let log = AttachmentMeta {
            size: 20_002,
            url: format!("{}/player.log", server.uri()),
            ..attachment("player.log", Some("text/plain; charset=utf-8"))
        };

        let resolved = resolve(&log, &modalities(&["text"]), &reqwest::Client::new())
            .await
            .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Text {
                label: "[attachment 1400000000000000001: player.log]".to_string(),
                content: format!("[first 3 bytes removed]\n{tail}"),
            }
        );
    }

    #[tokio::test]
    async fn other_label_contains_the_size_and_the_content_type() {
        let archive = attachment("archive.zip", Some("application/zip"));

        let resolved = resolve(&archive, &modalities(&["text"]), &reqwest::Client::new())
            .await
            .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Other {
                label: "[attachment 1400000000000000001: archive.zip, 2048 bytes, application/zip]"
                    .to_string(),
            }
        );
    }

    #[tokio::test]
    async fn other_label_without_a_content_type_says_unknown() {
        let blob = attachment("blob", None);

        let resolved = resolve(&blob, &modalities(&["text"]), &reqwest::Client::new())
            .await
            .unwrap();

        assert_eq!(
            resolved,
            ResolvedAttachment::Other {
                label: "[attachment 1400000000000000001: blob, 2048 bytes, unknown]".to_string(),
            }
        );
    }
}
