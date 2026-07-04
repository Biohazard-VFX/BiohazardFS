//! Linux FUSE adapter for BiohazardFS.
//!
//! This first slice is intentionally read-only and source-backed: it mounts a
//! safe virtual view of an existing workspace/source directory. It gives the
//! product a real mount path while the daemon/server cache and writeback layers
//! are still evolving.

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod unix;

#[cfg(target_os = "linux")]
pub use unix::{MountConfig, ReadOnlyWorkspaceFs, WorkspaceIndex, mount_read_only_workspace};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuseErrorKind {
    UnsupportedPlatform,
    InvalidSource,
    InvalidMountpoint,
    SourceTraversal,
    Io,
}

#[derive(Debug)]
pub struct FuseError {
    kind: FuseErrorKind,
    message: String,
    source: Option<std::io::Error>,
}

impl FuseError {
    pub fn new(kind: FuseErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn io(kind: FuseErrorKind, message: impl Into<String>, source: std::io::Error) -> Self {
        Self {
            kind,
            message: message.into(),
            source: Some(source),
        }
    }

    pub fn kind(&self) -> &FuseErrorKind {
        &self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for FuseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for FuseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|error| error as &(dyn std::error::Error + 'static))
    }
}

pub type Result<T> = std::result::Result<T, FuseError>;

pub fn validate_mount_inputs(source: &Path, mountpoint: &Path) -> Result<(PathBuf, PathBuf)> {
    let canonical_source = std::fs::canonicalize(source).map_err(|error| {
        FuseError::io(
            FuseErrorKind::InvalidSource,
            "source workspace root does not exist or cannot be resolved",
            error,
        )
    })?;
    let source_metadata = std::fs::metadata(&canonical_source).map_err(|error| {
        FuseError::io(
            FuseErrorKind::InvalidSource,
            "source workspace root cannot be inspected",
            error,
        )
    })?;
    if !source_metadata.is_dir() {
        return Err(FuseError::new(
            FuseErrorKind::InvalidSource,
            "source workspace root must be a directory",
        ));
    }

    let canonical_mountpoint = std::fs::canonicalize(mountpoint).map_err(|error| {
        FuseError::io(
            FuseErrorKind::InvalidMountpoint,
            "mountpoint does not exist or cannot be resolved",
            error,
        )
    })?;
    let mount_metadata = std::fs::metadata(&canonical_mountpoint).map_err(|error| {
        FuseError::io(
            FuseErrorKind::InvalidMountpoint,
            "mountpoint cannot be inspected",
            error,
        )
    })?;
    if !mount_metadata.is_dir() {
        return Err(FuseError::new(
            FuseErrorKind::InvalidMountpoint,
            "mountpoint must be a directory",
        ));
    }
    if canonical_source == canonical_mountpoint
        || canonical_source.starts_with(&canonical_mountpoint)
        || canonical_mountpoint.starts_with(&canonical_source)
    {
        return Err(FuseError::new(
            FuseErrorKind::InvalidMountpoint,
            "source workspace root and mountpoint must not overlap",
        ));
    }

    Ok((canonical_source, canonical_mountpoint))
}

#[cfg(not(target_os = "linux"))]
pub fn mount_read_only_workspace(_config: MountConfig) -> Result<()> {
    Err(FuseError::new(
        FuseErrorKind::UnsupportedPlatform,
        "BiohazardFS FUSE mounts are currently supported only on Linux platforms",
    ))
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone)]
pub struct MountConfig {
    pub source: PathBuf,
    pub mountpoint: PathBuf,
    pub foreground: bool,
}
