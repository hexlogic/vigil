//! Receiving the import form: its fields, and the file that comes with it, kept on disk.

use axum::extract::Multipart;
use axum::http::StatusCode;

use crate::AppState;
use crate::upload::{Spooled, Upload, UploadError};

/// What was typed in the form.
#[derive(Default)]
pub(super) struct Typed {
    pub(super) urls: String,
    pub(super) format: String,
    pub(super) every: String,
    pub(super) allow_duplicates: bool,
}

/// The fields of the form, and the file or pasted text it holds.
pub(super) struct Received {
    pub(super) typed: Typed,
    pub(super) file: Option<Spooled>,
}

pub(super) enum Problem {
    Message(String),
    TooLarge,
    Server(String),
}

pub(super) async fn receive(
    state: &AppState,
    mut multipart: Multipart,
) -> Result<Received, Problem> {
    let mut typed = Typed::default();
    let mut file = None;
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                // A body over the limit, or one that was cut off.
                return Err(if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    Problem::TooLarge
                } else {
                    Problem::Message(error.body_text())
                });
            }
        };
        match field.name().unwrap_or_default() {
            "file" => {
                let mut upload = Upload::new(&state.web.upload_dir, state.web.max_upload_bytes)
                    .map_err(|e| Problem::Server(e.to_string()))?;
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => match upload.push(&chunk).await {
                            Ok(()) => {}
                            Err(UploadError::TooLarge { .. }) => return Err(Problem::TooLarge),
                            Err(UploadError::Io(error)) => {
                                return Err(Problem::Server(error.to_string()));
                            }
                        },
                        Ok(None) => break,
                        Err(error) => {
                            return Err(if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                                Problem::TooLarge
                            } else {
                                Problem::Message(error.body_text())
                            });
                        }
                    }
                }
                let spooled = upload
                    .finish()
                    .await
                    .map_err(|e| Problem::Server(e.to_string()))?;
                // A browser sends an empty file part when none was chosen.
                if spooled.len > 0 {
                    file = Some(spooled);
                }
            }
            name => {
                let name = name.to_owned();
                let text = field
                    .text()
                    .await
                    .map_err(|e| Problem::Message(e.body_text()))?;
                match name.as_str() {
                    "urls" => typed.urls = text,
                    "format" => typed.format = text.trim().to_owned(),
                    "every" => typed.every = text.trim().to_owned(),
                    "allow_duplicates" => typed.allow_duplicates = true,
                    _ => {}
                }
            }
        }
    }
    Ok(Received { typed, file })
}
