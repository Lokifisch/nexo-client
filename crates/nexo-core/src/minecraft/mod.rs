//! Everything between "the user clicked Play" and "the JVM is running":
//! version metadata, the download pipeline, Fabric resolution, and command
//! construction.

pub mod download;
pub mod fabric;
pub mod install;
pub mod launch;
pub mod meta;

pub use download::{DownloadTask, Downloader, Progress};
pub use install::Installer;
pub use launch::{DEFAULT_MEMORY_MB, LaunchOptions, Launcher};
pub use meta::{ManifestVersion, VersionData, VersionManifest};
