//! Parallel, resumable-by-skipping file downloader.
//!
//! A fresh install is ~1,000 small asset objects plus a few dozen libraries.
//! Fetched sequentially that is dominated by round-trip latency, not
//! bandwidth — running many in flight at once is the single biggest lever on
//! how fast an install feels. [`DEFAULT_CONCURRENCY`] is capped well below
//! "as many as possible" because Mojang's CDN starts refusing connections
//! under very aggressive parallelism.

use crate::error::{Error, IoContext, Result};
use futures::stream::{self, StreamExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc::UnboundedSender;

pub const DEFAULT_CONCURRENCY: usize = 16;

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest: PathBuf,
    /// Verified after download when present.
    pub sha1: Option<String>,
    /// Used to decide whether an existing file can be skipped.
    pub size: u64,
}

/// Progress updates for the UI. Sent over a channel rather than a callback so
/// the download can run on the tokio runtime while iced renders on its own
/// thread.
#[derive(Debug, Clone)]
pub enum Progress {
    /// What phase we're in, e.g. "Downloading libraries".
    Stage(String),
    Advanced {
        completed: usize,
        total: usize,
    },
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Downloader {
    http: reqwest::Client,
    concurrency: usize,
}

impl Downloader {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            concurrency: DEFAULT_CONCURRENCY,
        }
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    /// Runs every task, skipping files already present and intact.
    ///
    /// Fails on the first error rather than continuing: a half-installed
    /// instance that launches into a crash is worse than a clear failure.
    pub async fn run(
        &self,
        tasks: Vec<DownloadTask>,
        progress: Option<&UnboundedSender<Progress>>,
    ) -> Result<()> {
        let total = tasks.len();
        let completed = Arc::new(AtomicUsize::new(0));

        if let Some(tx) = progress {
            let _ = tx.send(Progress::Advanced { completed: 0, total });
        }

        let results = stream::iter(tasks)
            .map(|task| {
                let http = self.http.clone();
                let completed = Arc::clone(&completed);
                async move {
                    let result = download_one(&http, &task).await;
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    (result, done)
                }
            })
            .buffer_unordered(self.concurrency);

        futures::pin_mut!(results);

        while let Some((result, done)) = results.next().await {
            result?;
            if let Some(tx) = progress {
                let _ = tx.send(Progress::Advanced {
                    completed: done,
                    total,
                });
            }
        }

        Ok(())
    }
}

async fn download_one(http: &reqwest::Client, task: &DownloadTask) -> Result<()> {
    if is_present(task).await {
        return Ok(());
    }

    if let Some(parent) = task.dest.parent() {
        tokio::fs::create_dir_all(parent).await.ctx(parent)?;
    }

    let bytes = http
        .get(&task.url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    if let Some(expected) = &task.sha1 {
        let actual = crate::util::sha1_hex(&bytes);
        if &actual != expected {
            return Err(Error::invalid(format!(
                "{} failed its checksum (got {actual}, expected {expected})",
                task.url
            )));
        }
    }

    // Write to a sibling temp file and rename, so an interrupted download
    // can't leave a truncated jar that looks complete on the next run.
    let temp = task.dest.with_extension("part");
    tokio::fs::write(&temp, &bytes).await.ctx(&temp)?;
    tokio::fs::rename(&temp, &task.dest).await.ctx(&task.dest)?;
    Ok(())
}

/// Cheap "already downloaded?" check. Size comparison rather than hashing:
/// re-hashing every asset on every launch would cost seconds of disk I/O for
/// a case that essentially never happens once a file is written atomically.
async fn is_present(task: &DownloadTask) -> bool {
    let Ok(meta) = tokio::fs::metadata(&task.dest).await else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    // Size 0 in the manifest means "unknown", so fall back to existence.
    task.size == 0 || meta.len() == task.size
}
