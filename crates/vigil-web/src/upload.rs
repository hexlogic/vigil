//! Keeping an uploaded file on disk while it is read.
//!
//! A backup can be large, and only some of it is read, so it is written to a temporary file
//! next to the data and read from there. The file is deleted when the value is dropped,
//! whatever happens in between.

use std::io;
use std::path::Path;

use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;

/// Why a file could not be kept.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UploadError {
    /// More bytes arrived than the limit.
    #[error("the upload is larger than the {limit} byte limit")]
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// The disk could not be written.
    #[error("the upload could not be kept: {0}")]
    Io(#[from] io::Error),
}

/// A file being received.
pub(crate) struct Upload {
    named: NamedTempFile,
    writer: tokio::fs::File,
    len: u64,
    limit: u64,
}

/// A file that was received in full.
pub(crate) struct Spooled {
    named: NamedTempFile,
    /// Its size in bytes.
    pub(crate) len: u64,
}

impl Upload {
    /// Start a file in `dir` that may grow to `limit` bytes.
    pub(crate) fn new(dir: &Path, limit: u64) -> io::Result<Self> {
        let named = NamedTempFile::new_in(dir)?;
        let writer = tokio::fs::File::from_std(named.reopen()?);
        Ok(Self {
            named,
            writer,
            len: 0,
            limit,
        })
    }

    /// Add the next bytes. The limit is counted here, on what actually arrives: a declared
    /// length can be wrong.
    pub(crate) async fn push(&mut self, chunk: &[u8]) -> Result<(), UploadError> {
        self.len += chunk.len() as u64;
        if self.len > self.limit {
            return Err(UploadError::TooLarge { limit: self.limit });
        }
        self.writer.write_all(chunk).await?;
        Ok(())
    }

    /// The end of the file.
    pub(crate) async fn finish(mut self) -> io::Result<Spooled> {
        self.writer.flush().await?;
        Ok(Spooled {
            named: self.named,
            len: self.len,
        })
    }
}

impl Spooled {
    /// A reader at the start of the file.
    pub(crate) fn reader(&self) -> io::Result<std::fs::File> {
        self.named.reopen()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::{Upload, UploadError};

    #[tokio::test]
    async fn a_file_is_kept_counted_and_removed_when_dropped()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut upload = Upload::new(dir.path(), 10)?;
        upload.push(b"hello").await?;
        upload.push(b" you").await?;
        let spooled = upload.finish().await?;
        assert_eq!(spooled.len, 9);
        let mut text = String::new();
        spooled.reader()?.read_to_string(&mut text)?;
        assert_eq!(text, "hello you");
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
        drop(spooled);
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn more_than_the_limit_is_refused_and_leaves_nothing_behind()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut upload = Upload::new(dir.path(), 4)?;
        upload.push(b"abc").await?;
        assert!(matches!(
            upload.push(b"de").await,
            Err(UploadError::TooLarge { limit: 4 })
        ));
        drop(upload);
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
        Ok(())
    }
}
