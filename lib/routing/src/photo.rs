pub const MAX_SLACK_PHOTO_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlackFile {
    pub id: String,
    pub name: String,
    pub mimetype: String,
    pub download_url: String,
    pub size: u64,
}

pub fn accepted_slack_photos(files: &[SlackFile]) -> Result<Vec<&SlackFile>, String> {
    let mut photos = Vec::new();
    for file in files {
        if slack_photo_extension(&file.mimetype).is_none() {
            let label = if file.name.is_empty() {
                file.mimetype.as_str()
            } else {
                file.name.as_str()
            };
            return Err(format!("not executed: unsupported file type: {label}"));
        }
        if file.size > MAX_SLACK_PHOTO_BYTES {
            return Err(format!("not executed: photo exceeds 20 MiB: {}", file.name));
        }
        if file.id.is_empty()
            || !file
                .id
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        {
            return Err("not executed: photo is missing a Slack file id".to_owned());
        }
        if !slack_file_url_is_downloadable(&file.download_url) {
            return Err("not executed: photo URL is not a Slack file URL".to_owned());
        }
        photos.push(file);
    }
    Ok(photos)
}

pub fn slack_file_url_is_downloadable(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    host == "files.slack.com" || host.ends_with(".slack.com")
}

fn slack_photo_extension(mimetype: &str) -> Option<&'static str> {
    match mimetype.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/heic" => Some("heic"),
        "image/heif" => Some("heif"),
        "image/bmp" => Some("bmp"),
        "image/tiff" | "image/tif" => Some("tiff"),
        _ => None,
    }
}

pub fn slack_photo_store_path(thread_ts: &str, photo: &SlackFile) -> Result<String, String> {
    let extension = slack_photo_extension(&photo.mimetype)
        .ok_or_else(|| "not executed: unsupported file type".to_owned())?;
    if photo.id.is_empty()
        || !photo
            .id
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err("not executed: photo is missing a Slack file id".to_owned());
    }
    let safe_thread: String = thread_ts
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '.' {
                character
            } else {
                '_'
            }
        })
        .collect();
    Ok(std::env::temp_dir()
        .join("pingme-photos")
        .join(safe_thread)
        .join(format!("{}.{extension}", photo.id))
        .display()
        .to_string())
}

pub fn prompt_with_local_photos(text: &str, paths: &[String]) -> String {
    let text = text.trim();
    if paths.is_empty() {
        return text.to_owned();
    }
    let listed = paths.join("\n");
    if text.is_empty() {
        listed
    } else {
        format!("{text}\n{listed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{ThreadAction, ThreadMessage, thread_action};

    fn slack_photo(id: &str, name: &str, mimetype: &str) -> SlackFile {
        SlackFile {
            id: id.to_owned(),
            name: name.to_owned(),
            mimetype: mimetype.to_owned(),
            download_url: format!("https://files.slack.com/files-pri/T/{id}/{name}"),
            size: 1024,
        }
    }

    #[test]
    fn operator_png_file_share_is_a_prompt() {
        let message = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("1757221923.123456"),
            user_id: Some("U_TEST_OPERATOR"),
            text: "",
            has_subtype: true,
            is_bot: false,
            event_ts: Some("1757221924.000000"),
            files: vec![slack_photo("F123PNG", "shot.png", "image/png")],
        };

        assert_eq!(
            ThreadAction::Prompt(String::new()),
            thread_action(&message, "U_TEST_OPERATOR", "codex",)
        );
    }

    #[test]
    fn operator_jpeg_caption_is_a_prompt() {
        let message = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("1757221923.123456"),
            user_id: Some("U_TEST_OPERATOR"),
            text: "what is wrong here",
            has_subtype: false,
            is_bot: false,
            event_ts: Some("1757221924.000000"),
            files: vec![slack_photo("F123JPG", "photo.jpg", "image/jpeg")],
        };

        assert_eq!(
            ThreadAction::Prompt("what is wrong here".to_owned()),
            thread_action(&message, "U_TEST_OPERATOR", "codex",)
        );
    }

    #[test]
    fn operator_pdf_in_session_thread_is_rejected() {
        let message = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("1757221923.123456"),
            user_id: Some("U_TEST_OPERATOR"),
            text: "read this",
            has_subtype: true,
            is_bot: false,
            event_ts: Some("1757221924.000000"),
            files: vec![slack_photo("F123PDF", "notes.pdf", "application/pdf")],
        };

        assert_eq!(
            ThreadAction::Reject("not executed: unsupported file type: notes.pdf".to_owned()),
            thread_action(&message, "U_TEST_OPERATOR", "codex",)
        );
    }

    #[test]
    fn file_share_without_files_is_ignored() {
        let message = ThreadMessage {
            channel_id: "C_TEST_CONTROL",
            thread_ts: Some("1757221923.123456"),
            user_id: Some("U_TEST_OPERATOR"),
            text: "",
            has_subtype: true,
            is_bot: false,
            event_ts: Some("1757221924.000000"),
            files: Vec::new(),
        };

        assert_eq!(
            ThreadAction::Ignore,
            thread_action(&message, "U_TEST_OPERATOR", "codex",)
        );
    }

    #[test]
    fn prompt_with_photos_appends_local_paths() {
        assert_eq!(
            "/tmp/pingme-photos/1757221923.123456/F123PNG.png",
            prompt_with_local_photos(
                "",
                &["/tmp/pingme-photos/1757221923.123456/F123PNG.png".to_owned()]
            )
        );
        assert_eq!(
            "what is wrong here\n/tmp/a.png\n/tmp/b.jpg",
            prompt_with_local_photos(
                "what is wrong here",
                &["/tmp/a.png".to_owned(), "/tmp/b.jpg".to_owned()]
            )
        );
    }

    #[test]
    fn photo_store_path_uses_file_id_and_extension() {
        let path = slack_photo_store_path(
            "1757221923.123456",
            &slack_photo("F123PNG", "shot.png", "image/png"),
        )
        .unwrap();
        assert!(
            path.ends_with("pingme-photos/1757221923.123456/F123PNG.png"),
            "{path}"
        );
    }
}
