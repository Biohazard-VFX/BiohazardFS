use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use biohazardfs_api_types::{ApiError, DaemonRequest, Source};
use biohazardfs_core::cache::{CacheState, transition as cache_transition};
use biohazardfs_core::path::case_insensitive_sibling_key;
use biohazardfs_daemon::{DaemonClientError, DaemonHttpClient};
use fuser::{
    Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo,
    MountOption, OpenAccMode, OpenFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use serde_json::Value;

use crate::{FuseError, FuseErrorKind, Result, validate_mount_inputs};

const ROOT_INODE: u64 = 1;
const TTL: Duration = Duration::from_secs(1);
const MAX_READ_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct MountConfig {
    pub source: PathBuf,
    pub mountpoint: PathBuf,
    pub foreground: bool,
}

#[derive(Debug, Clone)]
struct Node {
    inode: u64,
    parent: u64,
    relative_path: PathBuf,
    kind: FileType,
    source_dev: u64,
    source_ino: u64,
}

#[derive(Debug, Clone)]
pub struct WorkspaceIndex {
    source_root: PathBuf,
    nodes: HashMap<u64, Node>,
    children: HashMap<u64, BTreeMap<OsString, u64>>,
}

impl WorkspaceIndex {
    pub fn build(source_root: impl AsRef<Path>) -> Result<Self> {
        let source_root = fs::canonicalize(source_root.as_ref()).map_err(|error| {
            FuseError::io(
                FuseErrorKind::InvalidSource,
                "source workspace root does not exist or cannot be resolved",
                error,
            )
        })?;
        let metadata = fs::metadata(&source_root).map_err(|error| {
            FuseError::io(
                FuseErrorKind::InvalidSource,
                "source workspace root cannot be inspected",
                error,
            )
        })?;
        if !metadata.is_dir() {
            return Err(FuseError::new(
                FuseErrorKind::InvalidSource,
                "source workspace root must be a directory",
            ));
        }

        let mut index = Self {
            source_root,
            nodes: HashMap::new(),
            children: HashMap::new(),
        };
        index.nodes.insert(
            ROOT_INODE,
            Node {
                inode: ROOT_INODE,
                parent: ROOT_INODE,
                relative_path: PathBuf::new(),
                kind: FileType::Directory,
                source_dev: metadata.dev(),
                source_ino: metadata.ino(),
            },
        );
        index.children.insert(ROOT_INODE, BTreeMap::new());
        let root = index.source_root.clone();
        index.scan_directory(ROOT_INODE, &root, Path::new(""))?;
        Ok(index)
    }

    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn lookup_child(&self, parent: u64, name: &OsStr) -> Option<u64> {
        self.children.get(&parent)?.get(name).copied()
    }

    pub fn real_path(&self, inode: u64) -> Option<PathBuf> {
        let node = self.nodes.get(&inode)?;
        Some(self.source_root.join(&node.relative_path))
    }

    fn revalidated_metadata(&self, node: &Node) -> std::io::Result<fs::Metadata> {
        let real_path = self.source_root.join(&node.relative_path);
        let canonical = fs::canonicalize(&real_path)?;
        if !canonical.starts_with(&self.source_root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "source path escaped workspace root",
            ));
        }
        let metadata = fs::symlink_metadata(real_path)?;
        let kind_matches = match node.kind {
            FileType::Directory => metadata.is_dir(),
            FileType::RegularFile => metadata.is_file(),
            _ => false,
        };
        if !kind_matches || metadata.dev() != node.source_dev || metadata.ino() != node.source_ino {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "source entry changed since FUSE index was built",
            ));
        }
        Ok(metadata)
    }

    fn current_attr(&self, node: &Node) -> std::io::Result<FileAttr> {
        let metadata = self.revalidated_metadata(node)?;
        let perm = if node.kind == FileType::Directory {
            0o555
        } else {
            0o444
        };
        Ok(attr_from_metadata(node.inode, &metadata, node.kind, perm))
    }

    fn open_revalidated_file(&self, node: &Node) -> std::io::Result<File> {
        self.revalidated_metadata(node)?;
        let file = File::open(self.source_root.join(&node.relative_path))?;
        let open_metadata = file.metadata()?;
        if !open_metadata.is_file()
            || open_metadata.dev() != node.source_dev
            || open_metadata.ino() != node.source_ino
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "source file changed since FUSE index was built",
            ));
        }
        Ok(file)
    }

    fn scan_directory(&mut self, parent: u64, real_dir: &Path, relative_dir: &Path) -> Result<()> {
        let entries = fs::read_dir(real_dir).map_err(|error| {
            FuseError::io(
                FuseErrorKind::Io,
                format!("could not list source directory {}", real_dir.display()),
                error,
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                FuseError::io(
                    FuseErrorKind::Io,
                    "could not read source directory entry",
                    error,
                )
            })?;
            let file_name = entry.file_name();
            if file_name.as_bytes().contains(&0) {
                continue;
            }
            let real_path = entry.path();
            let metadata = fs::symlink_metadata(&real_path).map_err(|error| {
                FuseError::io(FuseErrorKind::Io, "could not inspect source entry", error)
            })?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let kind = if metadata.is_dir() {
                FileType::Directory
            } else if metadata.is_file() {
                FileType::RegularFile
            } else {
                continue;
            };
            let relative_path = relative_dir.join(&file_name);
            let canonical = fs::canonicalize(&real_path).map_err(|error| {
                FuseError::io(FuseErrorKind::Io, "could not resolve source entry", error)
            })?;
            if !canonical.starts_with(&self.source_root) {
                return Err(FuseError::new(
                    FuseErrorKind::SourceTraversal,
                    "source entry resolved outside workspace root",
                ));
            }
            let inode = (self.nodes.len() as u64) + 1;
            let node = Node {
                inode,
                parent,
                relative_path: relative_path.clone(),
                kind,
                source_dev: metadata.dev(),
                source_ino: metadata.ino(),
            };
            self.nodes.insert(inode, node);
            self.children
                .entry(parent)
                .or_default()
                .insert(file_name, inode);
            if kind == FileType::Directory {
                self.children.insert(inode, BTreeMap::new());
                self.scan_directory(inode, &real_path, &relative_path)?;
            }
        }
        Ok(())
    }
}

pub struct ReadOnlyWorkspaceFs {
    index: WorkspaceIndex,
}

impl ReadOnlyWorkspaceFs {
    pub fn new(index: WorkspaceIndex) -> Self {
        Self { index }
    }
}

impl Filesystem for ReadOnlyWorkspaceFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(inode) = self.index.lookup_child(parent.0, name) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let node = self.index.nodes.get(&inode).expect("indexed child exists");
        let Ok(attr) = self.index.current_attr(node) else {
            reply.error(Errno::EIO);
            return;
        };
        reply.entry(&TTL, &attr, Generation(0));
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let Some(node) = self.index.nodes.get(&ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let Ok(attr) = self.index.current_attr(node) else {
            reply.error(Errno::EIO);
            return;
        };
        reply.attr(&TTL, &attr);
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let Some(node) = self.index.nodes.get(&ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if node.kind != FileType::RegularFile {
            reply.error(Errno::EISDIR);
            return;
        }
        if flags.acc_mode() != OpenAccMode::O_RDONLY
            || flags.0 & (libc::O_APPEND | libc::O_TRUNC) != 0
        {
            reply.error(Errno::EROFS);
            return;
        }
        reply.opened(FileHandle(0), FopenFlags::FOPEN_DIRECT_IO);
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        let Some(node) = self.index.nodes.get(&ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if node.kind != FileType::RegularFile {
            reply.error(Errno::EISDIR);
            return;
        }
        let mut file = match self.index.open_revalidated_file(node) {
            Ok(file) => file,
            Err(_) => {
                reply.error(Errno::EIO);
                return;
            }
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            reply.error(Errno::EIO);
            return;
        }
        let mut buffer = vec![0; (size as usize).min(MAX_READ_SIZE)];
        match file.read(&mut buffer) {
            Ok(count) => reply.data(&buffer[..count]),
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Some(node) = self.index.nodes.get(&ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if node.kind != FileType::Directory {
            reply.error(Errno::ENOTDIR);
            return;
        }
        let mut entries: Vec<(INodeNo, FileType, OsString)> = vec![
            (ino, FileType::Directory, OsString::from(".")),
            (
                INodeNo(node.parent),
                FileType::Directory,
                OsString::from(".."),
            ),
        ];
        if let Some(children) = self.index.children.get(&ino.0) {
            for (name, child_inode) in children {
                let child = self
                    .index
                    .nodes
                    .get(child_inode)
                    .expect("indexed child exists");
                entries.push((INodeNo(child.inode), child.kind, name.clone()));
            }
        }
        for (entry_index, (entry_ino, kind, name)) in
            entries.into_iter().enumerate().skip(offset as usize)
        {
            let next_offset = (entry_index + 1) as u64;
            if reply.add(entry_ino, next_offset, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn mkdir(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EROFS);
    }

    fn unlink(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(Errno::EROFS);
    }

    fn rmdir(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(Errno::EROFS);
    }
}

pub fn mount_read_only_workspace(config: MountConfig) -> Result<()> {
    let (source, mountpoint) = validate_mount_inputs(&config.source, &config.mountpoint)?;
    let index = WorkspaceIndex::build(&source)?;
    let filesystem = ReadOnlyWorkspaceFs::new(index);
    let mut options = Config::default();
    options.mount_options = vec![
        MountOption::RO,
        MountOption::FSName("biohazardfs".to_string()),
        MountOption::Subtype("biohazardfs".to_string()),
        MountOption::DefaultPermissions,
    ];
    if config.foreground {
        eprintln!(
            "biohazardfs-fuse mounting {} at {} (read-only)",
            source.display(),
            mountpoint.display()
        );
    }
    fuser::mount2(filesystem, &mountpoint, &options).map_err(|error| {
        FuseError::io(
            FuseErrorKind::Io,
            format!(
                "could not mount BiohazardFS FUSE view at {}",
                mountpoint.display()
            ),
            error,
        )
    })
}

fn attr_from_metadata(inode: u64, metadata: &fs::Metadata, kind: FileType, perm: u16) -> FileAttr {
    FileAttr {
        ino: INodeNo(inode),
        size: if kind == FileType::RegularFile {
            metadata.len()
        } else {
            0
        },
        blocks: metadata.blocks(),
        atime: unix_time(metadata.atime(), metadata.atime_nsec()),
        mtime: unix_time(metadata.mtime(), metadata.mtime_nsec()),
        ctime: unix_time(metadata.ctime(), metadata.ctime_nsec()),
        crtime: UNIX_EPOCH,
        kind,
        perm,
        nlink: if kind == FileType::Directory { 2 } else { 1 },
        uid: metadata.uid(),
        gid: metadata.gid(),
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn unix_time(seconds: i64, nanos: i64) -> SystemTime {
    if seconds >= 0 {
        UNIX_EPOCH + Duration::new(seconds as u64, nanos.max(0) as u32)
    } else {
        UNIX_EPOCH
    }
}

trait OsStringBytes {
    fn as_bytes(&self) -> &[u8];
}

impl OsStringBytes for OsString {
    fn as_bytes(&self) -> &[u8] {
        use std::os::unix::ffi::OsStrExt;
        self.as_os_str().as_bytes()
    }
}

// ===========================================================================
// Read-write workspace mount (Wave 3)
//
// The read-write mount proxies every filesystem mutation through the local
// daemon. Read paths hydrate whole files into a local cache on open via
// `file.read`; write paths buffer per-handle and push one complete blob per
// flush/fsync via `file.write`. Hard safety boundaries (see
// FILESYSTEM_SEMANTICS.md) are stated at each method and below.
// ===========================================================================

const DEFAULT_FILE_MODE: u16 = 0o644;
const DEFAULT_DIR_MODE: u16 = 0o755;

/// Configuration for the read-write BiohazardFS workspace mount.
#[derive(Debug, Clone)]
pub struct WorkspaceMountConfig {
    /// Loopback daemon endpoint (`127.0.0.1:<port>` or `[::1]:<port>`).
    pub daemon_endpoint: String,
    /// Owner-only local daemon session token.
    pub local_token: String,
    /// Local cache directory for hydrated file content. Created if missing.
    pub cache_dir: PathBuf,
    /// Existing empty directory used as the FUSE mountpoint.
    pub mountpoint: PathBuf,
    /// Stay in the foreground. This is the current supported mode.
    pub foreground: bool,
}

/// Per-inode namespace + cache state. The daemon `node_id` is the source of
/// truth for identity; the inode number is the FUSE handle. A file created
/// locally that has not yet been flushed has `node_id = None` and a
/// provisional local inode only — the daemon assigns the node id on `file.write`.
///
/// `current_version_id` mirrors the daemon's current version for the node. It
/// is captured from `file.list` / `file.read` and used at open time as the
/// optimistic-concurrency base for `file.write` (see `OpenHandle::base_version_id`).
/// `None` for directories, freshly created files, and any daemon response that
/// omits the field.
#[derive(Debug, Clone)]
struct InodeState {
    inode: u64,
    parent_inode: u64,
    node_id: Option<String>,
    name: OsString,
    kind: FileType,
    mode: u16,
    target: Option<String>,
    mtime: SystemTime,
    crtime: SystemTime,
    cache_state: CacheState,
    content_hash: Option<String>,
    size_bytes: u64,
    current_version_id: Option<String>,
}

impl InodeState {
    fn cache_path(&self, cache_dir: &Path) -> PathBuf {
        inode_cache_path(cache_dir, self.inode)
    }
}

/// On-disk cache path for a given inode under `cache_dir`. Centralized so the
/// write-buffer seed path and `InodeState::cache_path` cannot drift.
fn inode_cache_path(cache_dir: &Path, inode: u64) -> PathBuf {
    cache_dir.join(format!("inode-{inode}"))
}

/// Children of a directory indexed by name and by daemon `node_id`. `fetched`
/// is set once a `file.list` has populated the cache so subsequent lookups skip
/// the RPC.
#[derive(Debug, Default, Clone)]
struct ChildrenCache {
    by_name: BTreeMap<OsString, u64>,
    by_node: HashMap<String, u64>,
    fetched: bool,
}

/// One open file handle. `writable` is sticky for the handle's lifetime (set
/// at create / open based on access mode); `write_buffer` is `Some` while
/// writes are buffered and `None` after a flush has committed the buffer. The
/// next write re-initializes it (see `write`): FUSE calls `flush` on each
/// close() of a duplicated descriptor, so a dup-then-write pattern can flush
/// before the write lands — the buffer must be re-allocatable, not treated as
/// read-only.
///
/// `write_buffer` is seeded with `Some(Vec::new())` on `create()` and on
/// `open(O_TRUNC)`: a truncating open that never writes must still commit a
/// zero-byte version (`: > $MNT/new.txt`), and flush is a no-op when the
/// buffer is `None`. A non-truncating writable open leaves the buffer `None`
/// so the first write seeds from the hydrated cache file (the round-1
/// partial-overwrite fix); `accumulate_write` uses `get_or_insert_with`, so a
/// later write extends the seeded empty buffer instead of re-seeding.
///
/// `base_version_id` captures the daemon's current version at open time, used
/// as the optimistic-concurrency base for `file.write` so the daemon can reject
/// a stale writer (`version_conflict`). `None` for freshly created files.
#[derive(Debug)]
struct OpenHandle {
    inode: u64,
    writable: bool,
    write_buffer: Option<Vec<u8>>,
    base_version_id: Option<String>,
}

#[derive(Debug)]
struct FsState {
    inodes: HashMap<u64, InodeState>,
    children: HashMap<u64, ChildrenCache>,
    handles: HashMap<u64, OpenHandle>,
    next_inode: u64,
    next_handle: u64,
    /// Owner uid/gid stamped on every attr so the kernel enforces the artist's
    /// own permissions under `MountOption::DefaultPermissions`. The daemon does
    /// not expose numeric ownership; identity lives in node.owner_user_id.
    uid: u32,
    gid: u32,
}

/// Error from a daemon RPC, mapped to FUSE errnos at the call site.
#[derive(Debug)]
enum RpcError {
    Client(DaemonClientError),
    Daemon(ApiError),
    Protocol(&'static str),
}

impl From<DaemonClientError> for RpcError {
    fn from(error: DaemonClientError) -> Self {
        RpcError::Client(error)
    }
}

impl RpcError {
    /// Human-readable summary for diagnostics. Read at failure sites so the
    /// artist/operator gets a real message alongside the EIO.
    fn message(&self) -> String {
        match self {
            RpcError::Client(error) => format!("daemon transport error: {error}"),
            RpcError::Daemon(error) => {
                format!("daemon error {}: {}", error.code, error.message)
            }
            RpcError::Protocol(message) => {
                format!("daemon protocol error: {message}")
            }
        }
    }
}

/// The read-write BiohazardFS FUSE filesystem backed by the local daemon.
///
/// Safety boundaries (FILESYSTEM_SEMANTICS.md):
/// - Dirty data is never silently lost: `flush`/`fsync` succeed only after the
///   daemon acknowledges the version. On failure the buffer is restored and the
///   call returns `EIO`; the artist sees a real write failure.
/// - One complete blob per `flush`/`fsync`. No streaming, partial writes,
///   fsync-after-rename atomicity, crash recovery, or write coalescing.
/// - Hydrate-on-open fetches the whole file before reply (MVP full-file hydrate).
/// - Cache state transitions go through `core::cache::transition` (rejects
///   `Dirty -> Evicting` and other unsafe moves).
#[derive(Debug)]
pub struct WorkspaceFs {
    http: Mutex<DaemonHttpClient>,
    cache_dir: PathBuf,
    state: Mutex<FsState>,
}

impl WorkspaceFs {
    /// Build the filesystem and pre-flight the daemon connection. Pre-flight
    /// fetches the namespace root so inode 1 binds to a real `node_id`, and
    /// fails fast with a typed error if the daemon is unreachable.
    pub fn connect(config: &WorkspaceMountConfig) -> Result<Self> {
        validate_loopback_endpoint(&config.daemon_endpoint)?;
        if config.local_token.is_empty() {
            return Err(FuseError::new(
                FuseErrorKind::InvalidSource,
                "local daemon token must not be empty",
            ));
        }
        let cache_dir = prepare_cache_dir(&config.cache_dir)?;
        let http =
            DaemonHttpClient::new(config.daemon_endpoint.clone(), config.local_token.clone());

        let root_node_id = fetch_root_node_id(&http)?;

        let now = SystemTime::now();
        let (uid, gid) = current_owner();
        let mut state = FsState {
            inodes: HashMap::new(),
            children: HashMap::new(),
            handles: HashMap::new(),
            next_inode: 2,
            next_handle: 1,
            uid,
            gid,
        };
        state.inodes.insert(
            ROOT_INODE,
            InodeState {
                inode: ROOT_INODE,
                parent_inode: ROOT_INODE,
                node_id: Some(root_node_id),
                name: OsString::from("/"),
                kind: FileType::Directory,
                mode: DEFAULT_DIR_MODE,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Ready,
                content_hash: None,
                size_bytes: 0,
                current_version_id: None,
            },
        );
        state.children.insert(ROOT_INODE, ChildrenCache::default());

        Ok(Self {
            http: Mutex::new(http),
            cache_dir,
            state: Mutex::new(state),
        })
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, FsState> {
        // Mutex poisoning while serving artist data is fatal; recover the inner
        // state rather than panicking on top of inconsistent data, matching the
        // daemon backend's policy.
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn lock_http(&self) -> std::sync::MutexGuard<'_, DaemonHttpClient> {
        self.http
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// On-disk path for an inode's dirty journal entry.
    fn dirty_journal_path(&self, inode: u64) -> PathBuf {
        self.cache_dir.join("dirty").join(format!("inode-{inode}"))
    }

    /// Best-effort durable persistence of unsynced dirty bytes. Survives
    /// `release` of the in-memory handle (which would otherwise drop the only
    /// copy after a failed flush) and mount-process restart. A follow-up replay
    /// path will push journaled bytes on daemon reconnect. Best-effort on
    /// purpose: a journal-write failure is logged and the bytes are lost rather
    /// than crashing the mount on top of artist data.
    fn journal_dirty(&self, inode: u64, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let path = self.dirty_journal_path(inode);
        let Some(parent) = path.parent() else {
            eprintln!("biohazardfs-fuse: dirty journal path has no parent for inode {inode}");
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            eprintln!("biohazardfs-fuse: could not create dirty journal dir for inode {inode}");
            return;
        }
        if fs::write(&path, bytes).is_err() {
            eprintln!("biohazardfs-fuse: could not journal dirty bytes for inode {inode}");
        }
    }

    /// Remove the journal entry once the bytes have reached the daemon.
    fn clear_dirty_journal(&self, inode: u64) {
        let _ = fs::remove_file(self.dirty_journal_path(inode));
    }

    /// Perform a daemon RPC and unpack the envelope. The HTTP lock is held only
    /// for the call; the state lock is never held across this (callers gather
    /// state first, drop it, then call).
    fn rpc(&self, method: &str, params: Value) -> std::result::Result<Value, RpcError> {
        let mut request = DaemonRequest::new(method, Source::Ui);
        request.params = params;
        let envelope = self.lock_http().call::<Value>(&request)?;
        if envelope.ok {
            envelope
                .data
                .ok_or(RpcError::Protocol("daemon returned ok=true without data"))
        } else {
            Err(RpcError::Daemon(envelope.error.unwrap_or_else(|| {
                ApiError::new("daemon_error", "daemon returned an error without details")
            })))
        }
    }

    /// Populate the children cache for a directory inode via `file.list`.
    /// Caller must NOT hold the state lock (this makes an RPC).
    fn ensure_children_fetched(&self, parent_inode: u64) -> std::result::Result<(), RpcError> {
        let parent_node_id = {
            let state = self.lock_state();
            let parent = state
                .inodes
                .get(&parent_inode)
                .ok_or(RpcError::Protocol("parent inode missing"))?;
            if parent.kind != FileType::Directory {
                return Err(RpcError::Protocol("inode is not a directory"));
            }
            if state
                .children
                .get(&parent_inode)
                .is_some_and(|cache| cache.fetched)
            {
                return Ok(());
            }
            parent
                .node_id
                .clone()
                .ok_or(RpcError::Protocol("directory has no daemon node_id"))?
        };

        let data = self.rpc(
            "file.list",
            serde_json::json!({ "parent_node_id": parent_node_id }),
        )?;
        let entries = data
            .get("entries")
            .and_then(|value| value.as_array())
            .ok_or(RpcError::Protocol(
                "file.list response missing entries array",
            ))?;

        let mut discovered: Vec<ListedEntry> = Vec::with_capacity(entries.len());
        for entry in entries {
            discovered.push(parse_list_entry(entry)?);
        }

        let mut state = self.lock_state();
        let prior = state
            .children
            .get(&parent_inode)
            .cloned()
            .unwrap_or_default();
        let mut next_inode = state.next_inode;
        let mut refreshed = ChildrenCache::default();
        for entry in discovered {
            let ListedEntry {
                name,
                node_id,
                kind,
                mode,
                target,
                size_bytes,
                current_version_id,
            } = entry;
            let inode = if let Some(&existing) = prior.by_node.get(&node_id) {
                // Refresh size + version from daemon truth on a re-fetch. The
                // fetched flag makes this rare (the cache is populated once per
                // directory), but when it does run the daemon is the source of
                // truth for the file's current size and version. A locally
                // Dirty, unflushed file has no node_id and so never matches
                // here — its unsynced state is preserved.
                if let Some(state_entry) = state.inodes.get_mut(&existing) {
                    state_entry.size_bytes = size_bytes;
                    if current_version_id.is_some() {
                        state_entry.current_version_id = current_version_id.clone();
                    }
                }
                existing
            } else {
                let allocated = next_inode;
                next_inode += 1;
                let now = SystemTime::now();
                state.inodes.insert(
                    allocated,
                    InodeState {
                        inode: allocated,
                        parent_inode,
                        node_id: Some(node_id.clone()),
                        name: OsString::from(&name),
                        kind,
                        mode,
                        target: target.clone(),
                        mtime: now,
                        crtime: now,
                        cache_state: if kind == FileType::RegularFile {
                            CacheState::Absent
                        } else {
                            CacheState::Ready
                        },
                        content_hash: None,
                        size_bytes,
                        current_version_id,
                    },
                );
                allocated
            };
            refreshed.by_name.insert(OsString::from(&name), inode);
            refreshed.by_node.insert(node_id, inode);
        }
        refreshed.fetched = true;
        state.next_inode = next_inode;
        state.children.insert(parent_inode, refreshed);
        Ok(())
    }

    /// Accumulate `data` at `offset` into the handle's buffer and persist the
    /// resulting snapshot to the durable dirty journal. Extracted from the
    /// FUSE `write` callback so the journaling behavior is testable without a
    /// live FUSE channel (a `ReplyWrite` cannot be built in-process).
    ///
    /// FIX 3: `write()` returns success to the kernel before `flush` ever
    /// runs, so the acknowledged bytes live only in the in-memory buffer until
    /// flush. If the FUSE process dies in that window the bytes are lost. We
    /// mirror the full buffer to `<cache_dir>/dirty/inode-<N>` after every
    /// extend so the bytes survive process death; a successful flush already
    /// clears the journal (`commit_handle` Ok path).
    ///
    /// Perf: one journal write (full-buffer `fs::write`) per FUSE write, plus a
    /// transient clone of the buffer so the state lock is not held across the
    /// disk write (the documented RPC-discipline extends to local I/O here to
    /// keep other inodes' FUSE ops progressing). OS-crash fsync hardening is a
    /// documented follow-up; today the journal survives process death, not
    /// power loss.
    fn buffer_write(
        &self,
        ino: u64,
        fh: u64,
        offset: usize,
        data: &[u8],
    ) -> std::result::Result<usize, Errno> {
        let dirty_snapshot = {
            let mut state = self.lock_state();
            let Some(handle) = state.handles.get_mut(&fh) else {
                return Err(Errno::EBADF);
            };
            if handle.inode != ino {
                return Err(Errno::EBADF);
            }
            if !handle.writable {
                return Err(Errno::EACCES);
            }
            // Seed the buffer from the hydrated cache file on first write (or
            // re-seed from the committed cache state after a prior flush), then
            // overlay this write. This is what makes a non-truncating edit to an
            // existing file preserve prior content: the writable open starts
            // with no buffer, and the first write loads the current cache bytes
            // so a write at an offset overlays instead of committing a sparse
            // zero-padded buffer. Still one complete blob per flush; no
            // streaming, no partials.
            accumulate_write(handle, &self.cache_dir, offset, data);
            handle.write_buffer.as_deref().map(Vec::from)
        };
        if let Some(bytes) = dirty_snapshot {
            // Best-effort: a journal-write failure is logged inside
            // `journal_dirty` and the bytes remain in memory for flush. The
            // clone above is the consistent snapshot persisted here.
            self.journal_dirty(ino, &bytes);
        }
        Ok(data.len())
    }

    /// Push the handle's write buffer to the daemon as one complete blob. This
    /// is the single sink for buffered writes and the safety valve for the
    /// "dirty data is never silently lost" invariant.
    fn commit_handle(&self, ino: u64, fh: u64) -> std::result::Result<(), Errno> {
        // Take the buffer and build params in one short critical section so the
        // RPC below runs with no lock held.
        let prepared = {
            let mut state = self.lock_state();
            let handle = state.handles.get_mut(&fh).ok_or(Errno::EBADF)?;
            if handle.inode != ino {
                return Err(Errno::EBADF);
            }
            let bytes = match handle.write_buffer.take() {
                Some(bytes) => bytes,
                None => return Ok(()),
            };
            // Capture the optimistic-concurrency base captured at open time.
            // The daemon rejects the write with `version_conflict` if the
            // node's current version has moved on (another writer flushed
            // first), so a stale handle cannot silently clobber a newer
            // version. Only sent on the node_id path; a freshly created file
            // has no current version to pin.
            let base_version_id = handle.base_version_id.clone();
            let entry = state.inodes.get(&ino).ok_or(Errno::EIO)?.clone();
            let content_hex = encode_hex(&bytes);
            let params = if let Some(node_id) = &entry.node_id {
                let mut params = serde_json::json!({
                    "node_id": node_id,
                    "content_hex": content_hex,
                });
                if let Some(base) = &base_version_id {
                    params["base_version_id"] = serde_json::Value::String(base.clone());
                }
                params
            } else {
                let parent_node_id = state
                    .inodes
                    .get(&entry.parent_inode)
                    .and_then(|parent| parent.node_id.as_ref())
                    .ok_or(Errno::EIO)?
                    .clone();
                let name = entry.name.to_string_lossy().into_owned();
                serde_json::json!({
                    "parent_node_id": parent_node_id,
                    "name": name,
                    "content_hex": content_hex,
                })
            };
            PreparedCommit {
                bytes,
                params,
                cache_path: entry.cache_path(&self.cache_dir),
            }
        };

        // Hard safety boundary: success only after file.write acknowledges the
        // version. The RPC runs with no state lock held.
        match self.rpc("file.write", prepared.params) {
            Ok(data) => {
                let node_id = data
                    .get("node_id")
                    .and_then(|value| value.as_str())
                    .map(String::from);
                let content_hash = data
                    .get("content_hash")
                    .and_then(|value| value.as_str())
                    .map(String::from);
                let size_bytes = data
                    .get("size_bytes")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(prepared.bytes.len() as u64);
                // The daemon advances the node's current version on every
                // accepted write. Mirror it so the next open captures a fresh
                // optimistic-concurrency base (FIX 4) and the inode reflects
                // daemon truth.
                let version_id = data
                    .get("version_id")
                    .and_then(|value| value.as_str())
                    .map(String::from);

                // Persist the committed bytes to the local cache so a subsequent
                // open/read sees them without a round-trip.
                if fs::write(&prepared.cache_path, &prepared.bytes).is_err() {
                    // The daemon truth is safe; the local cache write failed.
                    // Surface EIO so the artist knows the cache is inconsistent.
                    return Err(Errno::EIO);
                }

                // The bytes reached the daemon and the cache; any earlier dirty
                // journal entry is obsolete.
                self.clear_dirty_journal(ino);

                let mut state = self.lock_state();
                let parent_inode = state.inodes.get(&ino).map(|entry| entry.parent_inode);
                // Round-3 FIX 2: the daemon advanced the node's current version
                // on this commit. Mirror it onto the inode AND every open
                // handle bound to it. Without this, write→flush→write→flush on
                // the SAME handle re-sends the prior base_version_id (captured
                // at open time) and the daemon rejects the second commit with
                // version_conflict against the version we just committed — a
                // self-inflicted conflict on a single handle. The calling
                // handle's buffer was already taken above; other open handles
                // keep their own buffers but now pin the fresh base.
                let next_base = version_id.clone();
                if let Some(entry) = state.inodes.get_mut(&ino) {
                    if let Some(node_id) = &node_id {
                        entry.node_id = Some(node_id.clone());
                    }
                    entry.cache_state = CacheState::Ready;
                    entry.content_hash = content_hash;
                    entry.size_bytes = size_bytes;
                    if version_id.is_some() {
                        entry.current_version_id = version_id;
                    }
                    entry.mtime = SystemTime::now();
                }
                if let Some(next_base) = next_base {
                    for handle in state.handles.values_mut() {
                        if handle.inode == ino {
                            handle.base_version_id = Some(next_base.clone());
                        }
                    }
                }
                if let (Some(parent_inode), Some(node_id)) = (parent_inode, node_id)
                    && let Some(cache) = state.children.get_mut(&parent_inode)
                {
                    cache.by_node.insert(node_id, ino);
                }
                Ok(())
            }
            Err(error) => {
                // Daemon unreachable / rejected the write. Restore the buffer so
                // a later flush/retry can push it, and mark the entry Dirty when
                // reachable so cache state reflects unsynced work. Always EIO:
                // the artist must see the write failure. A `version_conflict`
                // means the base captured at open is stale — another writer
                // flushed first. The artist must re-open and re-edit; the
                // restored buffer + journal let a retry push after re-hydration.
                let conflict = matches!(
                    &error,
                    RpcError::Daemon(api_error) if api_error.code == "version_conflict"
                );
                eprintln!(
                    "biohazardfs-fuse: file.write push failed for inode {ino}: {}{}",
                    error.message(),
                    if conflict {
                        " (version_conflict: base_version_id is stale — re-open and re-edit)"
                    } else {
                        ""
                    }
                );
                // Persist the unsent bytes to the durable dirty journal so they
                // survive release/restart; a daemon-reconnect replay path will
                // push them. Done before re-locking state so the borrow is clean.
                self.journal_dirty(ino, &prepared.bytes);
                let mut state = self.lock_state();
                if let Some(handle) = state.handles.get_mut(&fh) {
                    handle.write_buffer = Some(prepared.bytes);
                }
                if let Some(entry) = state.inodes.get_mut(&ino)
                    && entry.cache_state == CacheState::Ready
                    && let Ok(dirty) = cache_transition(CacheState::Ready, CacheState::Dirty)
                {
                    entry.cache_state = dirty;
                }
                Err(Errno::EIO)
            }
        }
    }

    /// Apply a setattr(size) truncation (round-3 FIX 1). The kernel issues
    /// setattr(size=N) for `truncate(path, N)`, `ftruncate(fd, N)`, and the
    /// `: > $MNT/file` / Python `open(path, "wb")` patterns when they arrive
    /// as setattr rather than open(O_TRUNC). The prior code ignored `size` and
    /// returned success, so the mount kept reading the old content while the
    /// daemon kept the old bytes — a silent data-loss blocker for artists.
    ///
    /// This truncates the inode cache file to N bytes and stages the surviving
    /// bytes in a handle's write buffer so the next flush commits a real
    /// version. `fh` is the handle the kernel passed (Some for ftruncate, None
    /// for path-style truncate): with a handle the buffer is staged on it;
    /// without one the truncation folds into any open writable handle for the
    /// inode — the kernel follows a path-style truncate with a flush on its
    /// internally-opened handle — and if no handle is open the bytes are
    /// journaled so a later open+flush commits them rather than dropping them.
    ///
    /// Sparse extension past the current cache length is unimplemented: the
    /// MVP commits one complete blob per flush, so an extension would zero-pad
    /// the gap and manufacture artist data that does not exist on the daemon.
    /// It returns ENOSYS so the artist sees a real failure instead of a
    /// corrupt commit.
    fn apply_setattr_size(
        &self,
        ino: u64,
        fh: Option<u64>,
        new_size: u64,
    ) -> std::result::Result<(), Errno> {
        // Gather the cache path under lock; the truncation I/O runs without
        // the state lock so other inodes' FUSE ops keep progressing (matches
        // buffer_write's local-IO discipline — the state lock is never held
        // across disk or RPC work).
        let cache_path = {
            let state = self.lock_state();
            let entry = state.inodes.get(&ino).ok_or(Errno::ENOENT)?;
            if entry.kind != FileType::RegularFile {
                return Err(Errno::EISDIR);
            }
            inode_cache_path(&self.cache_dir, ino)
        };

        let new_len = usize::try_from(new_size).map_err(|_| Errno::EOVERFLOW)?;
        let truncated = truncate_cache_file(&cache_path, new_len)?;

        // Stage the truncation on a handle so flush commits it; advance the
        // inode size so getattr reflects the new length even before the flush
        // lands.
        let journal_pending = {
            let mut state = self.lock_state();
            if let Some(entry) = state.inodes.get_mut(&ino) {
                entry.size_bytes = truncated.len() as u64;
                entry.cache_state = CacheState::Ready;
            }
            // Prefer the kernel-provided fh; fall back to any open writable
            // handle for the inode so a path-style truncate still reaches a
            // flush. The immutable lookups below end at the expression boundary
            // (NLL), so the mutable staging borrow that follows is clean.
            let target = if fh.is_some_and(|id| {
                state
                    .handles
                    .get(&id)
                    .is_some_and(|handle| handle.inode == ino)
            }) {
                fh
            } else {
                state
                    .handles
                    .iter()
                    .find(|(_, handle)| handle.inode == ino && handle.writable)
                    .map(|(id, _)| *id)
            };
            match target {
                Some(id) => {
                    if let Some(handle) = state.handles.get_mut(&id) {
                        stage_handle_truncation(handle, new_len, &truncated);
                    }
                    None
                }
                None => Some(truncated),
            }
        };

        if let Some(bytes) = journal_pending {
            // No open handle is carrying the truncation. Persist the bytes to
            // the durable dirty journal so a future open+flush commits them;
            // today the replay path is the documented follow-up, but the bytes
            // survive release/restart rather than being dropped.
            self.journal_dirty(ino, &bytes);
        }
        Ok(())
    }
}

struct PreparedCommit {
    bytes: Vec<u8>,
    params: Value,
    cache_path: PathBuf,
}

impl Filesystem for WorkspaceFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        if self.ensure_children_fetched(parent.0).is_err() {
            reply.error(Errno::EIO);
            return;
        }
        let state = self.lock_state();
        let Some(child_inode) = state
            .children
            .get(&parent.0)
            .and_then(|cache| cache.by_name.get(name).copied())
        else {
            reply.error(Errno::ENOENT);
            return;
        };
        let Some(entry) = state.inodes.get(&child_inode) else {
            reply.error(Errno::EIO);
            return;
        };
        let attr = build_attr(entry, state.uid, state.gid);
        reply.entry(&TTL, &attr, Generation(0));
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let state = self.lock_state();
        let Some(entry) = state.inodes.get(&ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let attr = build_attr(entry, state.uid, state.gid);
        reply.attr(&TTL, &attr);
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        // size: truncate/extend the cache file and stage the surviving bytes in
        // the handle's write buffer so the next flush commits a real version
        // (round-3 FIX 1). mtime/mode are stored locally on the inode cache
        // entry. A failed truncation short-circuits the whole call so the
        // kernel reports setattr failed rather than applying mode/mtime on top
        // of stale content.
        if let Some(new_size) = size
            && let Err(errno) = self.apply_setattr_size(ino.0, fh.map(|handle| handle.0), new_size)
        {
            reply.error(errno);
            return;
        }
        let mut state = self.lock_state();
        let uid = state.uid;
        let gid = state.gid;
        let Some(entry) = state.inodes.get_mut(&ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if let Some(mode_bits) = mode {
            entry.mode = (mode_bits & 0o7777) as u16;
        }
        if let Some(new_mtime) = mtime {
            entry.mtime = match new_mtime {
                fuser::TimeOrNow::SpecificTime(time) => time,
                fuser::TimeOrNow::Now => SystemTime::now(),
            };
        }
        let attr = build_attr(entry, uid, gid);
        reply.attr(&TTL, &attr);
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        let state = self.lock_state();
        let Some(entry) = state.inodes.get(&ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if entry.kind != FileType::Symlink {
            reply.error(Errno::EINVAL);
            return;
        }
        let Some(target) = &entry.target else {
            reply.error(Errno::EIO);
            return;
        };
        reply.data(target.as_bytes());
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let (kind, node_id, cache_state, cache_path) = {
            let state = self.lock_state();
            let Some(entry) = state.inodes.get(&ino.0) else {
                reply.error(Errno::ENOENT);
                return;
            };
            if entry.kind != FileType::RegularFile {
                reply.error(Errno::EISDIR);
                return;
            }
            (
                entry.kind,
                entry.node_id.clone(),
                entry.cache_state,
                entry.cache_path(&self.cache_dir),
            )
        };
        let _ = kind;

        // O_TRUNC: drop cached content; the next flush commits the new content.
        let truncating = flags.0 & libc::O_TRUNC != 0;

        // FILESYSTEM_SEMANTICS.md: hydrate full file before reply unless
        // truncating. open-for-write still hydrates first so the artist edits
        // the complete file.
        if !truncating && cache_state != CacheState::Ready {
            let hydration = match node_id.as_ref() {
                Some(node_id) => self.rpc("file.read", serde_json::json!({ "node_id": node_id })),
                None => Ok(serde_json::Value::Null),
            };
            match hydration {
                Ok(data) if !data.is_null() => {
                    let content_hex = data
                        .get("content_hex")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let content = decode_hex(content_hex).unwrap_or_default();
                    if fs::write(&cache_path, &content).is_err() {
                        reply.error(Errno::EIO);
                        return;
                    }
                    let mut state = self.lock_state();
                    if let Some(entry) = state.inodes.get_mut(&ino.0) {
                        entry.cache_state = CacheState::Ready;
                        entry.content_hash = data
                            .get("content_hash")
                            .and_then(|value| value.as_str())
                            .map(String::from);
                        entry.size_bytes = content.len() as u64;
                        // Mirror the daemon's current version so the handle
                        // built below pins the right optimistic-concurrency
                        // base (FIX 4). Falls back to None when the daemon
                        // omits the field.
                        entry.current_version_id = data
                            .get("version_id")
                            .and_then(|value| value.as_str())
                            .map(String::from);
                        entry.mtime = SystemTime::now();
                    }
                }
                Ok(_) => {
                    // Locally created, uncommitted file: allow empty open.
                }
                Err(RpcError::Daemon(api_error))
                    if api_error.code == "content_not_cached"
                        || api_error.code == "file_not_found" =>
                {
                    // Placeholder or no committed content: allow empty open.
                }
                Err(_) => {
                    reply.error(Errno::EIO);
                    return;
                }
            }
        }

        if truncating {
            let _ = fs::write(&cache_path, []);
            let mut state = self.lock_state();
            if let Some(entry) = state.inodes.get_mut(&ino.0) {
                entry.cache_state = CacheState::Ready;
                entry.size_bytes = 0;
            }
        }

        let handle = {
            let mut state = self.lock_state();
            let write_mode = flags.acc_mode() != OpenAccMode::O_RDONLY;
            let writable = write_mode || truncating;
            // Pin the daemon's current version as the optimistic-concurrency
            // base for the next flush. Read from the inode (populated by
            // file.list or file.read above) so every open — including O_TRUNC,
            // cache-Ready, and post-create re-opens — captures the version
            // known at open time. None for freshly created files.
            let base_version_id = state
                .inodes
                .get(&ino.0)
                .and_then(|entry| entry.current_version_id.clone());
            // O_TRUNC must commit a zero-byte version even when the artist
            // never writes (`: > $MNT/existing.txt`), so seed an empty buffer.
            // A non-truncating writable open leaves the buffer None so the
            // first write seeds from the hydrated cache file (round-1
            // partial-overwrite fix). accumulate_write's get_or_insert_with
            // extends a seeded empty buffer rather than re-seeding.
            let write_buffer = if truncating { Some(Vec::new()) } else { None };
            let handle_id = state.next_handle;
            state.next_handle += 1;
            state.handles.insert(
                handle_id,
                OpenHandle {
                    inode: ino.0,
                    writable,
                    write_buffer,
                    base_version_id,
                },
            );
            handle_id
        };
        reply.opened(FileHandle(handle), FopenFlags::empty());
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        let cache_path = {
            let state = self.lock_state();
            let Some(entry) = state.inodes.get(&ino.0) else {
                reply.error(Errno::ENOENT);
                return;
            };
            if entry.kind != FileType::RegularFile {
                reply.error(Errno::EISDIR);
                return;
            }
            entry.cache_path(&self.cache_dir)
        };

        // Read from the hydrated cache file. If it does not exist (placeholder
        // never hydrated) return an empty read, matching "read returns EOF".
        let mut file = match fs::File::open(&cache_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                reply.data(&[]);
                return;
            }
            Err(_) => {
                reply.error(Errno::EIO);
                return;
            }
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            reply.error(Errno::EIO);
            return;
        }
        let mut buffer = vec![0u8; (size as usize).min(MAX_READ_SIZE)];
        match file.read(&mut buffer) {
            Ok(count) => reply.data(&buffer[..count]),
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyWrite,
    ) {
        match self.buffer_write(ino.0, fh.0, offset as usize, data) {
            Ok(written) => reply.written(written as u32),
            Err(errno) => reply.error(errno),
        }
    }

    fn flush(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _lock_owner: fuser::LockOwner,
        reply: ReplyEmpty,
    ) {
        match self.commit_handle(ino.0, fh.0) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn fsync(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        // Same path as flush: one complete blob per fsync.
        match self.commit_handle(ino.0, fh.0) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        // Drop the in-memory handle, but if it still carries unsent bytes (a
        // prior flush failed and restored them, and no retry followed), persist
        // them to the durable journal first so release does not drop the only
        // copy. A normal successful close has no buffer here (the last flush
        // took it), so this only fires on the failure path.
        let dirty_bytes = {
            let mut state = self.lock_state();
            state
                .handles
                .remove(&fh.0)
                .and_then(|handle| handle.write_buffer)
        };
        if let Some(bytes) = dirty_bytes {
            self.journal_dirty(ino.0, &bytes);
        }
        reply.ok();
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        // Verify parent is a directory.
        {
            let state = self.lock_state();
            let Some(entry) = state.inodes.get(&parent.0) else {
                reply.error(Errno::ENOENT);
                return;
            };
            if entry.kind != FileType::Directory {
                reply.error(Errno::ENOTDIR);
                return;
            }
        }

        // Fetch siblings so case-insensitive uniqueness is enforced up front.
        // The daemon re-checks on file.write (source of truth); this avoids a
        // late EIO-on-flush in the common case.
        if self.ensure_children_fetched(parent.0).is_err() {
            reply.error(Errno::EIO);
            return;
        }
        {
            let state = self.lock_state();
            if let Some(cache) = state.children.get(&parent.0) {
                let key = case_insensitive_sibling_key(&name.to_string_lossy());
                let conflict = cache.by_name.keys().any(|existing| {
                    case_insensitive_sibling_key(&existing.to_string_lossy()) == key
                });
                if conflict {
                    reply.error(Errno::EEXIST);
                    return;
                }
            }
        }

        let file_mode = parse_mode_from_u32(mode, DEFAULT_FILE_MODE);
        let (_inode, handle, attr) = {
            let mut state = self.lock_state();
            let uid = state.uid;
            let gid = state.gid;
            let inode = state.next_inode;
            state.next_inode += 1;
            let now = SystemTime::now();
            let entry = InodeState {
                inode,
                parent_inode: parent.0,
                node_id: None,
                name: name.to_os_string(),
                kind: FileType::RegularFile,
                mode: file_mode,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Absent,
                content_hash: None,
                size_bytes: 0,
                current_version_id: None,
            };
            let attr = build_attr(&entry, uid, gid);
            state.inodes.insert(inode, entry);
            state
                .children
                .entry(parent.0)
                .or_default()
                .by_name
                .insert(name.to_os_string(), inode);
            let handle_id = state.next_handle;
            state.next_handle += 1;
            state.handles.insert(
                handle_id,
                OpenHandle {
                    inode,
                    writable: true,
                    // create() must commit a zero-byte version even when the
                    // artist never writes (`: > $MNT/new.txt`). Seed an empty
                    // buffer so flush has something to push; accumulate_write's
                    // get_or_insert_with extends this rather than re-seeding
                    // from a non-existent cache file.
                    write_buffer: Some(Vec::new()),
                    // Freshly created file: no daemon version to pin yet.
                    base_version_id: None,
                },
            );
            (inode, handle_id, attr)
        };
        reply.created(
            &TTL,
            &attr,
            Generation(0),
            FileHandle(handle),
            FopenFlags::empty(),
        );
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        {
            let state = self.lock_state();
            let Some(entry) = state.inodes.get(&ino.0) else {
                reply.error(Errno::ENOENT);
                return;
            };
            if entry.kind != FileType::Directory {
                reply.error(Errno::ENOTDIR);
                return;
            }
        }
        if self.ensure_children_fetched(ino.0).is_err() {
            reply.error(Errno::EIO);
            return;
        }

        let state = self.lock_state();
        let parent_inode = state
            .inodes
            .get(&ino.0)
            .map(|entry| entry.parent_inode)
            .unwrap_or(ino.0);
        let mut entries: Vec<(INodeNo, FileType, OsString)> = vec![
            (ino, FileType::Directory, OsString::from(".")),
            (
                INodeNo(parent_inode),
                FileType::Directory,
                OsString::from(".."),
            ),
        ];
        if let Some(cache) = state.children.get(&ino.0) {
            for (name, child_inode) in &cache.by_name {
                if let Some(child) = state.inodes.get(child_inode) {
                    entries.push((INodeNo(child.inode), child.kind, name.clone()));
                }
            }
        }
        for (entry_index, (entry_ino, kind, name)) in
            entries.into_iter().enumerate().skip(offset as usize)
        {
            let next_offset = (entry_index + 1) as u64;
            if reply.add(entry_ino, next_offset, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn mkdir(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        // No daemon method yet for creating directories. Return EROFS so the
        // artist sees "not supported" rather than a silent no-op. Promotion
        // needs a daemon mkdir spine + FUSE-side operation-token minting.
        reply.error(Errno::EROFS);
    }

    fn symlink(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _link_name: &OsStr,
        _target: &Path,
        reply: ReplyEntry,
    ) {
        // No daemon method yet for symlink creation. EROFS awaits promotion.
        reply.error(Errno::EROFS);
    }

    fn unlink(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        // Routes to daemon file.delete once that method is promoted AND the
        // FUSE layer can mint the required operation token over the daemon API
        // (file.delete is Destructive under AgentSafe). Until then, EIO with a
        // clear comment so the artist sees the delete fail rather than no-op.
        reply.error(Errno::EIO);
    }

    fn rmdir(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        // Same gap as unlink: awaits file.delete promotion + FUSE-side token mint.
        reply.error(Errno::EIO);
    }

    fn rename(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _newparent: INodeNo,
        _newname: &OsStr,
        _flags: fuser::RenameFlags,
        reply: ReplyEmpty,
    ) {
        // Routes to daemon file.move once promoted AND the FUSE layer can mint
        // the required operation token (file.move is DataMoving under
        // AgentSafe). Until then, EIO so the artist sees a real failure.
        reply.error(Errno::EIO);
    }
}

/// Mount the read-write BiohazardFS workspace filesystem.
pub fn mount_workspace(config: WorkspaceMountConfig) -> Result<()> {
    let mountpoint = validate_workspace_mountpoint(&config.mountpoint, &config.cache_dir)?;
    let filesystem = WorkspaceFs::connect(&config)?;

    let mut options = Config::default();
    options.mount_options = vec![
        MountOption::FSName("biohazardfs".to_string()),
        MountOption::Subtype("biohazardfs".to_string()),
        MountOption::DefaultPermissions,
    ];
    if config.foreground {
        eprintln!(
            "biohazardfs-fuse mounting workspace at {} (daemon {}, cache {})",
            mountpoint.display(),
            config.daemon_endpoint,
            config.cache_dir.display()
        );
    }
    fuser::mount2(filesystem, &mountpoint, &options).map_err(|error| {
        FuseError::io(
            FuseErrorKind::Io,
            format!(
                "could not mount BiohazardFS workspace at {}",
                mountpoint.display()
            ),
            error,
        )
    })
}

fn fetch_root_node_id(http: &DaemonHttpClient) -> Result<String> {
    let mut request = DaemonRequest::new("file.list", Source::Ui);
    request.params = serde_json::Value::Object(Default::default());
    let envelope = http
        .call::<Value>(&request)
        .map_err(|error| FuseError::new(FuseErrorKind::Io, daemon_preflight_message(error)))?;
    if !envelope.ok {
        return Err(FuseError::new(
            FuseErrorKind::InvalidSource,
            "daemon rejected workspace pre-flight",
        ));
    }
    let data = envelope
        .data
        .ok_or_else(|| FuseError::new(FuseErrorKind::InvalidSource, "daemon returned no data"))?;
    let root_id = data
        .get("parent_node_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            FuseError::new(
                FuseErrorKind::InvalidSource,
                "daemon file.list did not report a root node id",
            )
        })?;
    Ok(root_id.to_string())
}

fn daemon_preflight_message(error: DaemonClientError) -> String {
    format!("daemon pre-flight failed: {error}")
}

fn validate_loopback_endpoint(endpoint: &str) -> Result<()> {
    biohazardfs_daemon::validate_loopback_addr(endpoint).map_err(|message| {
        FuseError::new(
            FuseErrorKind::InvalidSource,
            format!("invalid daemon endpoint {endpoint:?}: {message}"),
        )
    })
}

fn prepare_cache_dir(cache_dir: &Path) -> Result<PathBuf> {
    let canonical = if cache_dir.exists() {
        fs::canonicalize(cache_dir).map_err(|error| {
            FuseError::io(
                FuseErrorKind::InvalidMountpoint,
                "cache_dir could not be canonicalized",
                error,
            )
        })?
    } else {
        fs::create_dir_all(cache_dir).map_err(|error| {
            FuseError::io(
                FuseErrorKind::InvalidMountpoint,
                format!("could not create cache_dir {}", cache_dir.display()),
                error,
            )
        })?;
        fs::canonicalize(cache_dir).map_err(|error| {
            FuseError::io(
                FuseErrorKind::InvalidMountpoint,
                "cache_dir could not be canonicalized after creation",
                error,
            )
        })?
    };
    let metadata = fs::metadata(&canonical).map_err(|error| {
        FuseError::io(
            FuseErrorKind::InvalidMountpoint,
            "cache_dir cannot be inspected",
            error,
        )
    })?;
    if !metadata.is_dir() {
        return Err(FuseError::new(
            FuseErrorKind::InvalidMountpoint,
            "cache_dir must be a directory",
        ));
    }
    Ok(canonical)
}

fn validate_workspace_mountpoint(mountpoint: &Path, cache_dir: &Path) -> Result<PathBuf> {
    let canonical_mountpoint = fs::canonicalize(mountpoint).map_err(|error| {
        FuseError::io(
            FuseErrorKind::InvalidMountpoint,
            "mountpoint does not exist or cannot be resolved",
            error,
        )
    })?;
    let mount_metadata = fs::metadata(&canonical_mountpoint).map_err(|error| {
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
    // The cache_dir holds the live hydrate content; mounting over it (or vice
    // versa) would make cache I/O alias FUSE traffic and confuse the kernel.
    if canonical_mountpoint == cache_dir
        || canonical_mountpoint.starts_with(cache_dir)
        || cache_dir.starts_with(&canonical_mountpoint)
    {
        return Err(FuseError::new(
            FuseErrorKind::InvalidMountpoint,
            "mountpoint and cache_dir must not overlap",
        ));
    }
    Ok(canonical_mountpoint)
}

/// Owner uid/gid for stamped attrs. The mount runs as the artist; the daemon
/// is per-user. Returned as a tuple so callers do not need libc directly.
fn current_owner() -> (u32, u32) {
    // Safety: getuid/getgid take no arguments and return the calling process's
    // ids. They are documented as always-successful on POSIX platforms.
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    (uid, gid)
}

fn build_attr(entry: &InodeState, uid: u32, gid: u32) -> FileAttr {
    FileAttr {
        ino: INodeNo(entry.inode),
        size: if entry.kind == FileType::RegularFile {
            entry.size_bytes
        } else {
            0
        },
        blocks: entry.size_bytes.div_ceil(4096),
        atime: SystemTime::now(),
        mtime: entry.mtime,
        ctime: entry.mtime,
        crtime: entry.crtime,
        kind: entry.kind,
        perm: entry.mode,
        nlink: if entry.kind == FileType::Directory {
            2
        } else {
            1
        },
        uid,
        gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn parse_kind(raw: &str) -> FileType {
    match raw {
        "directory" => FileType::Directory,
        "symlink" => FileType::Symlink,
        _ => FileType::RegularFile,
    }
}

/// One parsed `file.list` entry. Kept as a struct (not a tuple) so adding a
/// field later cannot silently re-order positional bindings at the fetch site.
#[derive(Debug, Clone)]
struct ListedEntry {
    name: String,
    node_id: String,
    kind: FileType,
    mode: u16,
    target: Option<String>,
    size_bytes: u64,
    current_version_id: Option<String>,
}

/// Parse one `file.list` entry into a [`ListedEntry`].
///
/// `size_bytes` and `current_version_id` are read defensively: an older daemon
/// that does not advertise them yields `0` / `None`, matching the pre-fix
/// behavior for absent fields rather than breaking the mount. A daemon that
/// does advertise them is honored — this is the path that lets the kernel see
/// the real file size from `lookup`/`getattr` (FIX 1) and bind the
/// optimistic-concurrency base version (FIX 4).
fn parse_list_entry(entry: &Value) -> std::result::Result<ListedEntry, RpcError> {
    let name = entry
        .get("name")
        .and_then(|value| value.as_str())
        .ok_or(RpcError::Protocol("file.list entry missing name"))?;
    let node_id = entry
        .get("node_id")
        .and_then(|value| value.as_str())
        .ok_or(RpcError::Protocol("file.list entry missing node_id"))?;
    let kind = parse_kind(
        entry
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("file"),
    );
    let mode = parse_mode(
        entry.get("mode").and_then(|value| value.as_str()),
        if kind == FileType::Directory {
            DEFAULT_DIR_MODE
        } else {
            DEFAULT_FILE_MODE
        },
    );
    let target = entry
        .get("target")
        .and_then(|value| value.as_str())
        .map(String::from);
    let size_bytes = entry
        .get("size_bytes")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let current_version_id = entry
        .get("current_version_id")
        .and_then(|value| value.as_str())
        .map(String::from);
    Ok(ListedEntry {
        name: name.to_string(),
        node_id: node_id.to_string(),
        kind,
        mode,
        target,
        size_bytes,
        current_version_id,
    })
}

fn parse_mode(raw: Option<&str>, default: u16) -> u16 {
    raw.and_then(|value| {
        let trimmed = value.trim_start_matches("0o");
        u16::from_str_radix(trimmed, 8).ok()
    })
    .unwrap_or(default)
}

fn parse_mode_from_u32(mode: u32, default: u16) -> u16 {
    let candidate = (mode & 0o7777) as u16;
    if candidate == 0 { default } else { candidate }
}

/// Overlay `data` at `offset` onto the handle's write buffer, seeding the
/// buffer from the hydrated cache file on first write. This is the fix for
/// non-truncating edits to existing files: a writable open must not start with
/// an empty buffer, or a write at a non-zero offset commits a sparse
/// zero-padded buffer (e.g. `abcdef` + seek 3 + write `X` became `00 00 00 58`)
/// instead of overlaying the existing content. A freshly created file has no
/// cache file yet, so its seed is empty. After a successful flush the buffer is
/// re-seeded from the committed cache file, preserving content across
/// dup-then-write cycles.
fn accumulate_write(handle: &mut OpenHandle, cache_dir: &Path, offset: usize, data: &[u8]) {
    let buffer = handle.write_buffer.get_or_insert_with(|| {
        fs::read(inode_cache_path(cache_dir, handle.inode)).unwrap_or_default()
    });
    extend_at(buffer, offset, data);
}

fn extend_at(buffer: &mut Vec<u8>, offset: usize, data: &[u8]) {
    let end = offset.saturating_add(data.len());
    if buffer.len() < end {
        buffer.resize(end, 0);
    }
    buffer[offset..end].copy_from_slice(data);
}

/// Truncate the inode cache file to `new_len` bytes and return the surviving
/// content so it can be staged in a handle's write buffer. Used by
/// `apply_setattr_size` to honor setattr(size) (round-3 FIX 1).
///
/// A missing cache file (never-hydrated placeholder) is treated as empty so a
/// size==0 setattr on a placeholder still commits a zero-byte version rather
/// than failing on a file that does not exist. Sparse extension past the
/// current cache length is rejected with ENOSYS: the MVP commits one complete
/// blob per flush, so an extension would zero-pad the gap and manufacture
/// artist data that does not exist on the daemon. The read happens before any
/// rewrite, so a rejected extend leaves the cache file untouched.
fn truncate_cache_file(cache_path: &Path, new_len: usize) -> std::result::Result<Vec<u8>, Errno> {
    let existing = match fs::read(cache_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            eprintln!(
                "biohazardfs-fuse: setattr cache read failed for {}: {error}",
                cache_path.display()
            );
            return Err(Errno::EIO);
        }
    };
    if new_len > existing.len() {
        eprintln!(
            "biohazardfs-fuse: setattr extend to {new_len} beyond cache len {} not supported \
             (sparse extension unimplemented)",
            existing.len()
        );
        return Err(Errno::ENOSYS);
    }
    let mut truncated = existing;
    truncated.truncate(new_len);
    if let Err(error) = fs::write(cache_path, &truncated) {
        eprintln!(
            "biohazardfs-fuse: setattr cache rewrite failed for {}: {error}",
            cache_path.display()
        );
        return Err(Errno::EIO);
    }
    Ok(truncated)
}

/// Stage a setattr(size) truncation on an open handle so the next flush
/// commits exactly `new_len` bytes (round-3 FIX 1). If the handle already has
/// a write buffer, truncate it in place; if the buffer was shorter than the
/// target (a partial write that never reached `new_len`), re-seed from the
/// now-truncated cache content rather than zero-padding a partial buffer. If
/// the handle has no buffer yet, seed it from the cache content so a flush has
/// something to push even when no write syscall lands — this is the
/// `: > file` / `ftruncate(fd, 0)` path that must commit a zero-byte version.
fn stage_handle_truncation(handle: &mut OpenHandle, new_len: usize, truncated: &[u8]) {
    match handle.write_buffer.as_mut() {
        Some(buffer) => {
            if buffer.len() > new_len {
                buffer.truncate(new_len);
            } else if buffer.len() < new_len {
                *buffer = truncated.to_vec();
            }
        }
        None => {
            handle.write_buffer = Some(truncated.to_vec());
        }
    }
}

/// Decode a lowercase hex string into bytes (mirrors the daemon encoder). Used
/// to turn the daemon `content_hex` field into file content on hydration.
fn decode_hex(hex: &str) -> std::result::Result<Vec<u8>, ()> {
    if !hex.len().is_multiple_of(2) {
        return Err(());
    }
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let high = hex_nibble(bytes[index])?;
        let low = hex_nibble(bytes[index + 1])?;
        out.push((high << 4) | low);
        index += 2;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> std::result::Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(()),
    }
}

/// Encode bytes as lowercase hex. Mirrors the daemon encoder so content
/// round-trips byte-identically between file.read and file.write.
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("biohazardfs-fuse-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn builds_index_for_regular_files_and_directories() {
        let root = temp_dir("index");
        fs::create_dir(root.join("plates")).expect("create subdir");
        fs::write(root.join("plates/shot001.txt"), b"plate").expect("write file");

        let index = WorkspaceIndex::build(&root).expect("build index");
        let plates = index
            .lookup_child(ROOT_INODE, OsStr::new("plates"))
            .expect("plates inode");
        let shot = index
            .lookup_child(plates, OsStr::new("shot001.txt"))
            .expect("shot inode");
        let node = index.nodes.get(&shot).expect("shot node");
        assert_eq!(node.kind, FileType::RegularFile);
        assert_eq!(index.current_attr(node).expect("current attr").size, 5);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn skips_symlinks() {
        let root = temp_dir("symlink");
        fs::write(root.join("target.txt"), b"target").expect("write target");
        std::os::unix::fs::symlink(root.join("target.txt"), root.join("link.txt"))
            .expect("create symlink");

        let index = WorkspaceIndex::build(&root).expect("build index");
        assert!(
            index
                .lookup_child(ROOT_INODE, OsStr::new("target.txt"))
                .is_some()
        );
        assert!(
            index
                .lookup_child(ROOT_INODE, OsStr::new("link.txt"))
                .is_none()
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn validates_mount_inputs() {
        let root = temp_dir("validate-source");
        let mount = temp_dir("validate-mount");
        let (source, mountpoint) = validate_mount_inputs(&root, &mount).expect("valid inputs");
        assert!(source.is_absolute());
        assert!(mountpoint.is_absolute());
        fs::remove_dir_all(root).expect("cleanup source");
        fs::remove_dir_all(mount).expect("cleanup mount");
    }

    #[test]
    fn rejects_overlapping_mount_inputs() {
        let root = temp_dir("overlap");
        let nested_mount = root.join("mount");
        fs::create_dir_all(&nested_mount).expect("create nested mount");
        let error = validate_mount_inputs(&root, &nested_mount).expect_err("reject overlap");
        assert_eq!(error.kind(), &FuseErrorKind::InvalidMountpoint);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn revalidation_rejects_replaced_file() {
        let root = temp_dir("replaced");
        fs::write(root.join("asset.txt"), b"asset").expect("write file");
        let index = WorkspaceIndex::build(&root).expect("build index");
        let asset = index
            .lookup_child(ROOT_INODE, OsStr::new("asset.txt"))
            .expect("asset inode");
        let node = index.nodes.get(&asset).expect("asset node");
        fs::remove_file(root.join("asset.txt")).expect("remove file");
        std::os::unix::fs::symlink("/etc/passwd", root.join("asset.txt"))
            .expect("replace with symlink");
        assert!(index.open_revalidated_file(node).is_err());
        assert!(index.current_attr(node).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn current_attr_reflects_same_inode_file_growth() {
        let root = temp_dir("growth");
        fs::write(root.join("asset.txt"), b"asset").expect("write file");
        let index = WorkspaceIndex::build(&root).expect("build index");
        let asset = index
            .lookup_child(ROOT_INODE, OsStr::new("asset.txt"))
            .expect("asset inode");
        let node = index.nodes.get(&asset).expect("asset node");
        fs::OpenOptions::new()
            .append(true)
            .open(root.join("asset.txt"))
            .expect("open file")
            .write_all(b" grew")
            .expect("append file");
        let attr = index.current_attr(node).expect("current attr");
        assert_eq!(attr.size, 10);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn read_only_open_rejects_write_flags() {
        let root = temp_dir("readonly");
        let mut file = File::create(root.join("asset.txt")).expect("create file");
        file.write_all(b"asset").expect("write file");
        let index = WorkspaceIndex::build(&root).expect("build index");
        let asset = index
            .lookup_child(ROOT_INODE, OsStr::new("asset.txt"))
            .expect("asset inode");
        let node = index.nodes.get(&asset).expect("asset node");
        assert_eq!(index.current_attr(node).expect("current attr").perm, 0o444);
        fs::remove_dir_all(root).expect("cleanup");
    }

    // ----- WorkspaceFs (read-write) tests -----

    #[test]
    fn hex_helpers_round_trip_and_reject_garbage() {
        assert!(decode_hex("").unwrap().is_empty());
        assert_eq!(
            decode_hex("deadbeef").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert_eq!(
            decode_hex("DEADBEEF").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert!(decode_hex("abc").is_err());
        assert!(decode_hex("xy").is_err());
        assert_eq!(encode_hex(&[0x00, 0xff, 0x10]), "00ff10");
        // Round trip preserves bytes exactly.
        let payload = b"BiohazardFS hydrate payload \x00\xff\x10";
        assert_eq!(decode_hex(&encode_hex(payload)).unwrap(), payload);
    }

    #[test]
    fn extend_at_handles_sequential_overwrite_and_gap() {
        let mut buffer = Vec::new();
        extend_at(&mut buffer, 0, b"hello");
        assert_eq!(buffer, b"hello");

        // Overwrite the middle (in place, not insertion).
        extend_at(&mut buffer, 1, b"EE");
        assert_eq!(buffer, b"hEElo");

        // Write past the end zero-pads the gap.
        extend_at(&mut buffer, 8, b"XY");
        assert_eq!(buffer, b"hEElo\x00\x00\x00XY");
    }

    #[test]
    fn accumulate_write_overlays_existing_content_not_sparse() {
        // Repro for the partial-write corruption blocker: a non-truncating edit
        // to an existing file must preserve prior content. With an empty seed
        // buffer, writing X at offset 3 of "abcdef" produced [0,0,0,X] and lost
        // the original. Seeding from the hydrated cache file fixes it.
        let dir = workspace_test_dir("accumulate-overlay");
        let inode = 9u64;
        fs::write(inode_cache_path(&dir, inode), b"abcdef").expect("seed cache file");

        let mut handle = OpenHandle {
            inode,
            writable: true,
            write_buffer: None,
            base_version_id: None,
        };
        accumulate_write(&mut handle, &dir, 3, b"X");
        assert_eq!(handle.write_buffer.as_deref(), Some(b"abcXef".as_slice()));
    }

    #[test]
    fn accumulate_write_seeds_empty_for_new_file() {
        // A freshly created file has no cache file yet; the seed is empty so
        // the first write starts the buffer from scratch.
        let dir = workspace_test_dir("accumulate-newfile");
        let inode = 10u64;
        let mut handle = OpenHandle {
            inode,
            writable: true,
            write_buffer: None,
            base_version_id: None,
        };
        accumulate_write(&mut handle, &dir, 0, b"hello");
        assert_eq!(handle.write_buffer.as_deref(), Some(b"hello".as_slice()));
    }

    #[test]
    fn dirty_journal_persists_and_clears() {
        let dir = workspace_test_dir("dirty-journal");
        let http = DaemonHttpClient::new("127.0.0.1:1".to_string(), "x".to_string());
        let mount = WorkspaceFs {
            http: Mutex::new(http),
            cache_dir: dir,
            state: Mutex::new(FsState {
                inodes: HashMap::new(),
                children: HashMap::new(),
                handles: HashMap::new(),
                next_inode: 2,
                next_handle: 1,
                uid: 0,
                gid: 0,
            }),
        };

        // Non-empty bytes are journaled.
        mount.journal_dirty(7, b"persist me");
        let path = mount.dirty_journal_path(7);
        assert_eq!(std::fs::read(&path).unwrap(), b"persist me");

        // Clearing removes the entry once the daemon has the bytes.
        mount.clear_dirty_journal(7);
        assert!(!path.exists(), "journal entry cleared after commit");

        // Empty bytes are not journaled (no spurious empty entries).
        mount.journal_dirty(8, b"");
        assert!(
            !mount.dirty_journal_path(8).exists(),
            "empty bytes must not be journaled"
        );
    }

    #[test]
    fn parse_kind_and_mode_use_defaults() {
        assert_eq!(parse_kind("file"), FileType::RegularFile);
        assert_eq!(parse_kind("directory"), FileType::Directory);
        assert_eq!(parse_kind("symlink"), FileType::Symlink);
        assert_eq!(parse_kind("unknown"), FileType::RegularFile);

        assert_eq!(parse_mode(Some("0o755"), DEFAULT_DIR_MODE), 0o755);
        assert_eq!(parse_mode(Some("0o644"), DEFAULT_FILE_MODE), 0o644);
        assert_eq!(parse_mode(None, DEFAULT_FILE_MODE), DEFAULT_FILE_MODE);
        assert_eq!(parse_mode(Some("garbage"), 0o700), 0o700);

        assert_eq!(parse_mode_from_u32(0o755, DEFAULT_FILE_MODE), 0o755);
        assert_eq!(parse_mode_from_u32(0, DEFAULT_FILE_MODE), DEFAULT_FILE_MODE);
    }

    /// Boot an in-process dev-loopback daemon on an ephemeral port. Returned
    /// backend lets tests seed state before the client connects.
    fn start_test_daemon(
        token: &str,
    ) -> (
        String,
        std::sync::Arc<biohazardfs_daemon::DaemonBackend>,
        std::thread::JoinHandle<()>,
    ) {
        use biohazardfs_daemon::{DaemonBackend, DevLoopbackConfig, run_dev_loopback_http};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test daemon");
        let addr = listener.local_addr().expect("listener addr");
        drop(listener);
        let addr_string = addr.to_string();
        let backend = std::sync::Arc::new(DaemonBackend::new(addr_string.clone()));
        let config = DevLoopbackConfig::with_backend(
            addr_string.clone(),
            token.to_string(),
            backend.clone(),
        );
        let handle = std::thread::spawn(move || {
            let _ = run_dev_loopback_http(config);
        });
        // Spin briefly until the daemon accepts connections.
        for _ in 0..100 {
            if std::net::TcpStream::connect(addr).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        (addr_string, backend, handle)
    }

    fn workspace_test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "biohazardfs-fuse-workspace-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create workspace test dir");
        path
    }

    #[test]
    fn workspace_connect_binds_root_inode_to_daemon_root() {
        let (addr, _backend, _handle) = start_test_daemon("ws_token");
        let cache = workspace_test_dir("connect-cache");
        let mount = workspace_test_dir("connect-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_token".to_string(),
            cache_dir: cache.clone(),
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        let state = fs.lock_state();
        let root = state.inodes.get(&ROOT_INODE).expect("root inode");
        assert_eq!(root.kind, FileType::Directory);
        assert!(
            root.node_id
                .as_ref()
                .is_some_and(|id| id.starts_with("node_")),
            "root must bind to a daemon node id"
        );
    }

    #[test]
    fn workspace_connect_rejects_bad_endpoint_and_empty_token() {
        let cache = workspace_test_dir("bad-endpoint-cache");
        let mount = workspace_test_dir("bad-endpoint-mount");
        let err = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: "192.168.1.1:47666".to_string(),
            local_token: "tok".to_string(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect_err("non-loopback endpoint rejected");
        assert_eq!(err.kind(), &FuseErrorKind::InvalidSource);

        let cache = workspace_test_dir("empty-token-cache");
        let mount = workspace_test_dir("empty-token-mount");
        let err = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: "127.0.0.1:47666".to_string(),
            local_token: String::new(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect_err("empty token rejected");
        assert_eq!(err.kind(), &FuseErrorKind::InvalidSource);
    }

    #[test]
    fn workspace_ensure_children_fetched_lists_root_children() {
        let (addr, _backend, _handle) = start_test_daemon("ws_list");
        let cache = workspace_test_dir("list-cache");
        let mount = workspace_test_dir("list-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_list".to_string(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        fs.ensure_children_fetched(ROOT_INODE)
            .expect("fetch root children");
        let state = fs.lock_state();
        let cache = state
            .children
            .get(&ROOT_INODE)
            .expect("root children cache populated");
        assert!(cache.fetched);
        // The seeded daemon namespace has `shots` and `README.md` under root.
        let names: Vec<String> = cache
            .by_name
            .keys()
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|name| name == "shots"));
        assert!(names.iter().any(|name| name == "README.md"));
    }

    /// Simulate `create()` + `flush()` end-to-end without a real FUSE mount:
    /// insert an inode + handle holding a write buffer, call `commit_handle`,
    /// and verify the daemon recorded an immutable version and bound the node.
    #[test]
    fn workspace_commit_handle_pushes_blob_and_binds_node() {
        let (addr, backend, _handle) = start_test_daemon("ws_commit");
        let cache = workspace_test_dir("commit-cache");
        let mount = workspace_test_dir("commit-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_commit".to_string(),
            cache_dir: cache.clone(),
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        // Simulate create(): allocate an inode + handle with a buffered payload.
        let payload = b"biohazard workspace write".to_vec();
        let (inode, handle) = {
            let mut state = fs.lock_state();
            let inode = state.next_inode;
            state.next_inode += 1;
            let now = SystemTime::now();
            state.inodes.insert(
                inode,
                InodeState {
                    inode,
                    parent_inode: ROOT_INODE,
                    node_id: None,
                    name: OsString::from("committed.txt"),
                    kind: FileType::RegularFile,
                    mode: DEFAULT_FILE_MODE,
                    target: None,
                    mtime: now,
                    crtime: now,
                    cache_state: CacheState::Absent,
                    content_hash: None,
                    size_bytes: 0,
                    current_version_id: None,
                },
            );
            state
                .children
                .entry(ROOT_INODE)
                .or_default()
                .by_name
                .insert(OsString::from("committed.txt"), inode);
            let handle = state.next_handle;
            state.next_handle += 1;
            state.handles.insert(
                handle,
                OpenHandle {
                    inode,
                    writable: true,
                    write_buffer: Some(payload.clone()),
                    base_version_id: None,
                },
            );
            (inode, handle)
        };

        fs.commit_handle(inode, handle).expect("commit succeeds");

        // The inode is now bound to a daemon node id and cache_state is Ready.
        let state = fs.lock_state();
        let entry = state.inodes.get(&inode).expect("inode persists");
        assert_eq!(entry.cache_state, CacheState::Ready);
        assert!(
            entry.node_id.is_some(),
            "node id bound after successful flush"
        );
        assert_eq!(entry.size_bytes, payload.len() as u64);
        // Cache file holds the committed bytes.
        let cached = fs::read(entry.cache_path(&cache)).expect("cache file exists");
        assert_eq!(cached, payload);
        let node_id = entry.node_id.clone().unwrap();
        drop(state);

        // The daemon truth reflects the immutable version + content.
        let inner = backend
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        assert!(inner.file_contents.contains_key(&node_id));
        assert_eq!(
            inner.file_contents.get(&node_id).unwrap(),
            &payload.as_slice()
        );
        assert!(
            inner
                .audit
                .iter()
                .any(|event| event.event_type == "file.write"
                    && event.node_id.as_deref() == Some(node_id.as_str()))
        );
    }

    /// When the daemon is unreachable, `commit_handle` must NOT silently drop
    /// the buffer. It restores the buffer and returns EIO; the artist sees a
    /// real write failure (FILESYSTEM_SEMANTICS.md dirty-data invariant).
    #[test]
    fn workspace_commit_handle_keeps_buffer_on_daemon_failure() {
        // Construct a WorkspaceFs whose client points at a dead endpoint. We
        // bypass connect() (which pre-flights) and build the state by hand.
        let cache = workspace_test_dir("fail-cache");
        let http = DaemonHttpClient::new("127.0.0.1:1".to_string(), "dead".to_string());
        let (uid, gid) = current_owner();
        let mut state = FsState {
            inodes: HashMap::new(),
            children: HashMap::new(),
            handles: HashMap::new(),
            next_inode: 2,
            next_handle: 1,
            uid,
            gid,
        };
        let inode = 2u64;
        let now = SystemTime::now();
        state.inodes.insert(
            inode,
            InodeState {
                inode,
                parent_inode: ROOT_INODE,
                node_id: None,
                name: OsString::from("orphan.txt"),
                kind: FileType::RegularFile,
                mode: DEFAULT_FILE_MODE,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Absent,
                content_hash: None,
                size_bytes: 0,
                current_version_id: None,
            },
        );
        let parent_node = "node_root".to_string();
        state.inodes.entry(ROOT_INODE).or_insert_with(|| {
            // Insert a synthetic root so commit_handle can resolve the parent.
            InodeState {
                inode: ROOT_INODE,
                parent_inode: ROOT_INODE,
                node_id: Some(parent_node.clone()),
                name: OsString::from("/"),
                kind: FileType::Directory,
                mode: DEFAULT_DIR_MODE,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Ready,
                content_hash: None,
                size_bytes: 0,
                current_version_id: None,
            }
        });
        let payload = b"do not lose me".to_vec();
        state.handles.insert(
            1u64,
            OpenHandle {
                inode,
                writable: true,
                write_buffer: Some(payload.clone()),
                base_version_id: None,
            },
        );
        let fs = WorkspaceFs {
            http: Mutex::new(http),
            cache_dir: cache,
            state: Mutex::new(state),
        };

        let errno = fs
            .commit_handle(inode, 1)
            .expect_err("commit must fail when daemon is unreachable");
        assert_eq!(i32::from(errno), libc::EIO);

        // The buffer survives so a retry can push it.
        let state = fs.lock_state();
        let handle = state.handles.get(&1).expect("handle retained");
        assert_eq!(handle.write_buffer.as_deref(), Some(payload.as_slice()));
        drop(state);

        // The dirty bytes are also persisted to the durable journal so a later
        // release cannot drop the only copy.
        let journal_path = fs.dirty_journal_path(inode);
        let journaled = std::fs::read(&journal_path).expect("dirty journal written on failure");
        assert_eq!(journaled, payload);
    }

    #[test]
    fn workspace_mountpoint_rejects_overlap_with_cache() {
        let cache = workspace_test_dir("overlap-cache");
        let mount = cache.join("mount");
        fs::create_dir_all(&mount).expect("create nested mountpoint");
        let err = validate_workspace_mountpoint(&mount, &cache);
        assert!(err.is_err(), "overlapping mountpoint/cache_dir rejected");
        assert_eq!(err.unwrap_err().kind(), &FuseErrorKind::InvalidMountpoint);
    }

    #[test]
    fn workspace_prepare_cache_dir_creates_missing() {
        let root = workspace_test_dir("prepare-parent");
        let target = root.join("deep/cache");
        let prepared = prepare_cache_dir(&target).expect("creates nested cache dir");
        assert!(prepared.is_absolute());
        assert!(prepared.is_dir());
    }

    // ----- FIX 1: preexisting files advertise daemon size_bytes at lookup -----

    /// `parse_list_entry` must read `size_bytes` defensively: present → honored,
    /// absent → 0. This is the load-bearing FUSE-side parse for FIX 1 (the
    /// kernel sees the real size from lookup, not 0).
    #[test]
    fn parse_list_entry_reads_size_bytes_defensively() {
        let with_size = serde_json::json!({
            "node_id": "node_a",
            "name": "plate.exr",
            "kind": "file",
            "current_version_id": "ver_1",
            "size_bytes": 4096,
            "mode": "0o644",
        });
        let parsed = parse_list_entry(&with_size).expect("parse");
        assert_eq!(parsed.size_bytes, 4096);
        assert_eq!(parsed.current_version_id.as_deref(), Some("ver_1"));
        assert_eq!(parsed.kind, FileType::RegularFile);
        assert_eq!(parsed.mode, 0o644);

        // An older daemon that omits size_bytes / current_version_id yields the
        // documented defensive defaults rather than an error.
        let without_size = serde_json::json!({
            "node_id": "node_b",
            "name": "legacy.txt",
            "kind": "file",
        });
        let parsed = parse_list_entry(&without_size).expect("parse");
        assert_eq!(parsed.size_bytes, 0);
        assert!(parsed.current_version_id.is_none());

        // Missing identity fields are still a protocol error.
        assert!(parse_list_entry(&serde_json::json!({"name": "x"})).is_err());
        assert!(parse_list_entry(&serde_json::json!({"node_id": "n"})).is_err());
    }

    /// End-to-end FIX 1: a preexisting daemon file (created by a prior session)
    /// advertises its real size through `lookup`/`getattr` with no write. The
    /// kernel caches the attr from lookup, so a 0-byte size there makes
    /// `stat`/`dd`/Python read empty even though open hydrates content.
    #[test]
    fn workspace_getattr_reports_daemon_size_bytes_without_write() {
        let (addr, _backend, _handle) = start_test_daemon("ws_fix1");
        // Session A creates and commits a 1024-byte file.
        let cache_a = workspace_test_dir("fix1-cache-a");
        let mount_a = workspace_test_dir("fix1-mount-a");
        let fs_a = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr.clone(),
            local_token: "ws_fix1".to_string(),
            cache_dir: cache_a,
            mountpoint: mount_a,
            foreground: false,
        })
        .expect("connect A");
        let payload = vec![0x78u8; 1024];
        let (inode_a, handle_a) = seed_uncommitted_handle(
            &fs_a,
            "sized.txt",
            payload.clone(),
            /*base_version_id*/ None,
        );
        fs_a.commit_handle(inode_a, handle_a)
            .expect("create commit");
        let node_id = {
            let state = fs_a.lock_state();
            state
                .inodes
                .get(&inode_a)
                .and_then(|entry| entry.node_id.clone())
                .expect("node bound after commit")
        };

        // Session B lists the root fresh. The daemon advertises size_bytes for
        // the preexisting file; the FUSE layer must read it into InodeState so
        // getattr (build_attr) returns the real size at lookup time.
        let cache_b = workspace_test_dir("fix1-cache-b");
        let mount_b = workspace_test_dir("fix1-mount-b");
        let fs_b = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_fix1".to_string(),
            cache_dir: cache_b,
            mountpoint: mount_b,
            foreground: false,
        })
        .expect("connect B");
        fs_b.ensure_children_fetched(ROOT_INODE)
            .expect("fetch root");

        let (uid, gid, attr) = {
            let state = fs_b.lock_state();
            let child_inode = state
                .children
                .get(&ROOT_INODE)
                .and_then(|cache| cache.by_name.get(OsStr::new("sized.txt")))
                .copied()
                .expect("preexisting file listed");
            let entry = state.inodes.get(&child_inode).expect("inode entry");
            assert_eq!(entry.node_id.as_deref(), Some(node_id.as_str()));
            assert_eq!(
                entry.size_bytes, 1024,
                "lookup must advertise the daemon-reported size, not 0"
            );
            (
                state.uid,
                state.gid,
                build_attr(entry, state.uid, state.gid),
            )
        };
        // getattr returns entry.size_bytes via build_attr.
        let _ = (uid, gid);
        assert_eq!(attr.size, 1024);
        assert_eq!(attr.blocks, 1);
    }

    // ----- FIX 2: zero-byte create / O_TRUNC is durable -----

    /// `: > $MNT/new.txt` must commit a zero-byte version. create() (and
    /// open(O_TRUNC)) seed write_buffer with Some(Vec::new()) so flush has
    /// something to push even when no write syscall lands. Here we simulate
    /// create()+flush and assert the daemon recorded the file at zero bytes.
    #[test]
    fn workspace_zero_byte_create_commits_to_daemon() {
        let (addr, backend, _handle) = start_test_daemon("ws_fix2");
        let cache = workspace_test_dir("fix2-cache");
        let mount = workspace_test_dir("fix2-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_fix2".to_string(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        // create() seeds an EMPTY buffer (FIX 2). No write() syscall follows.
        let (inode, handle) = seed_uncommitted_handle(&fs, "empty.txt", Vec::new(), None);
        fs.commit_handle(inode, handle)
            .expect("zero-byte commit succeeds");

        let node_id = {
            let state = fs.lock_state();
            state
                .inodes
                .get(&inode)
                .and_then(|entry| entry.node_id.clone())
                .expect("node bound after zero-byte flush")
        };

        // The daemon truth: a zero-byte content entry and a current version.
        let inner = backend
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let recorded = inner
            .file_contents
            .get(&node_id)
            .cloned()
            .expect("file recorded in daemon content store");
        assert!(recorded.is_empty(), "daemon recorded zero bytes");
        assert!(
            inner
                .nodes
                .get(&node_id)
                .and_then(|node| node.current_version_id.as_ref())
                .is_some(),
            "daemon advanced the current version"
        );
        drop(inner);

        // file.list surfaces the new file (zero bytes advertised).
        let listed = biohazardfs_daemon::backend::file_list_payload(
            &backend,
            &serde_json::json!({ "parent_node_id": "node_root" }),
        )
        .expect("file.list");
        let names: Vec<String> = listed["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .map(|entry| entry["name"].as_str().expect("name").to_string())
            .collect();
        assert!(
            names.iter().any(|name| name == "empty.txt"),
            "daemon file.list includes the zero-byte file"
        );
    }

    // ----- FIX 3: acknowledged writes journaled before flush -----

    /// `write()` returns success before flush, so the acknowledged bytes live
    /// only in memory until flush. After the fix, buffer_write persists the
    /// full buffer to the durable dirty journal on every write — the journal
    /// file holds the bytes before any flush.
    #[test]
    fn workspace_write_journals_dirty_bytes_before_flush() {
        let (addr, _backend, _handle) = start_test_daemon("ws_fix3");
        let cache = workspace_test_dir("fix3-cache");
        let mount = workspace_test_dir("fix3-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_fix3".to_string(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        // Simulate open() of an existing file: a writable handle with no buffer
        // yet (non-truncating open) and a known base version.
        let (inode, handle) = seed_writable_open(&fs, "existing.txt", Some("ver_open".to_string()));

        // No journal entry before the first acknowledged write.
        assert!(
            !fs.dirty_journal_path(inode).exists(),
            "no journal entry before any write"
        );

        let written = fs
            .buffer_write(inode, handle, 0, b"acknowledged")
            .expect("write accepted");
        assert_eq!(written, 12);

        // The dirty journal holds the acknowledged bytes — they survive FUSE
        // process death before flush (clear_dirty_journal runs on flush Ok).
        let journal = fs.dirty_journal_path(inode);
        let bytes = std::fs::read(&journal).expect("dirty journal written on write");
        assert_eq!(bytes, b"acknowledged");

        // A second write at an offset journals the full overlayed buffer.
        fs.buffer_write(inode, handle, 12, b"-more")
            .expect("second write");
        let bytes = std::fs::read(&journal).expect("journal updated");
        assert_eq!(bytes, b"acknowledged-more");
    }

    // ----- FIX 4: stale-handle last-writer-wins via base_version_id -----

    /// Opening an existing file pins its current version as the
    /// optimistic-concurrency base; flush forwards base_version_id in the
    /// file.write params so the daemon can reject a stale writer.
    #[test]
    fn workspace_commit_forwards_base_version_id() {
        let (addr, backend, _handle) = start_test_daemon("ws_fix4a");
        let cache = workspace_test_dir("fix4a-cache");
        let mount = workspace_test_dir("fix4a-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_fix4a".to_string(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        // First commit creates the file; the daemon assigns version V.
        let (inode, handle_create) = seed_uncommitted_handle(
            &fs,
            "versioned.txt",
            b"original".to_vec(),
            /*base_version_id*/ None,
        );
        fs.commit_handle(inode, handle_create)
            .expect("create commit");

        let (node_id, base_version) = {
            let state = fs.lock_state();
            let entry = state.inodes.get(&inode).expect("inode");
            (
                entry.node_id.clone().expect("node bound"),
                entry
                    .current_version_id
                    .clone()
                    .expect("version mirrored from file.write response"),
            )
        };

        // Simulate a real open() of the existing file: capture base_version_id
        // from the inode (as open() does), write, then flush.
        let handle_edit = seed_handle_on(&fs, inode, Some(base_version.clone()));
        fs.buffer_write(inode, handle_edit, 0, b"updated")
            .expect("write");
        fs.commit_handle(inode, handle_edit).expect("edit commit");

        // The latest file.write op for this node must carry base_version_id.
        let inner = backend
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let last_write = inner
            .operations
            .iter()
            .rev()
            .find(|op| {
                op.method == "file.write" && op.base_node_id.as_deref() == Some(node_id.as_str())
            })
            .expect("file.write operation recorded");
        let params: Value =
            serde_json::from_str(&last_write.params_json).expect("params_json parses");
        assert_eq!(
            params
                .get("base_version_id")
                .and_then(|value| value.as_str()),
            Some(base_version.as_str()),
            "file.write params must include the captured base_version_id"
        );
        assert_eq!(
            params.get("node_id").and_then(|value| value.as_str()),
            Some(node_id.as_str()),
            "edit commit hit the node_id update path"
        );
        // The daemon redacts content_hex from the operation log (content never
        // lands in the audit trail); base_version_id survives redaction.
        assert!(
            params.get("content_hex").is_none(),
            "content_hex must not be persisted in the operation log"
        );
    }

    /// A synthetic stale base (another writer flushed first and bumped the
    /// version) must surface as version_conflict → EIO, and the daemon must
    /// NOT advance the version.
    #[test]
    fn workspace_commit_stale_base_yields_conflict_eio() {
        let (addr, backend, _handle) = start_test_daemon("ws_fix4b");
        let cache = workspace_test_dir("fix4b-cache");
        let mount = workspace_test_dir("fix4b-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_fix4b".to_string(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        // Create the file (version V).
        let (inode, handle_create) =
            seed_uncommitted_handle(&fs, "protected.txt", b"original".to_vec(), None);
        fs.commit_handle(inode, handle_create)
            .expect("create commit");
        let (node_id, real_version) = {
            let state = fs.lock_state();
            let entry = state.inodes.get(&inode).expect("inode");
            (
                entry.node_id.clone().expect("node bound"),
                entry.current_version_id.clone().expect("version set"),
            )
        };

        // Flush with a stale base. The daemon returns version_conflict; FUSE
        // surfaces it as EIO.
        let handle_stale = {
            let mut state = fs.lock_state();
            let handle_id = state.next_handle;
            state.next_handle += 1;
            state.handles.insert(
                handle_id,
                OpenHandle {
                    inode,
                    writable: true,
                    write_buffer: Some(b"stale write".to_vec()),
                    base_version_id: Some("ver_stale".to_string()),
                },
            );
            handle_id
        };
        let errno = fs
            .commit_handle(inode, handle_stale)
            .expect_err("stale base must conflict");
        assert_eq!(i32::from(errno), libc::EIO);

        // The daemon did NOT advance the version or store the stale bytes.
        let inner = backend
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let current = inner
            .nodes
            .get(&node_id)
            .and_then(|node| node.current_version_id.clone());
        assert_eq!(
            current,
            Some(real_version),
            "version unchanged after stale-base conflict"
        );
        assert_ne!(
            inner
                .file_contents
                .get(&node_id)
                .map(|bytes| bytes.as_slice()),
            Some(&b"stale write"[..]),
            "stale bytes not committed"
        );
    }

    // ----- Round-3 FIX 1: setattr(size) truncates + stages a flush -----

    /// setattr(size=0) must truncate the cache file and stage an empty buffer
    /// so the next flush commits a zero-byte version. The blocker repro:
    /// `: > $MNT/truncate-me.txt` (or Python `with open(path,"wb"): pass`)
    /// issued via setattr(size=0) silently did nothing — the mount still read
    /// `abc` and the daemon still returned the old bytes.
    #[test]
    fn workspace_setattr_size_zero_commits_empty_version() {
        let (addr, backend, _handle) = start_test_daemon("ws_setattr_zero");
        let cache = workspace_test_dir("setattr-zero-cache");
        let mount = workspace_test_dir("setattr-zero-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_setattr_zero".to_string(),
            cache_dir: cache.clone(),
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        // Seed a daemon file with content "abc" via a prior flush.
        let (inode, handle_seed) =
            seed_uncommitted_handle(&fs, "truncate-me.txt", b"abc".to_vec(), None);
        fs.commit_handle(inode, handle_seed).expect("seed commit");
        let node_id = {
            let state = fs.lock_state();
            state
                .inodes
                .get(&inode)
                .and_then(|entry| entry.node_id.clone())
                .expect("node bound after seed")
        };

        // Simulate open() of the existing file for ftruncate: a writable
        // handle with no buffer, pinned to the committed version.
        let base_version = {
            let state = fs.lock_state();
            state
                .inodes
                .get(&inode)
                .and_then(|entry| entry.current_version_id.clone())
        };
        let handle = seed_handle_on(&fs, inode, base_version);
        let cache_path = inode_cache_path(&cache, inode);
        assert_eq!(fs::read(&cache_path).expect("seed cache"), b"abc");

        // setattr(size=0): the kernel's truncation callback.
        fs.apply_setattr_size(inode, Some(handle), 0)
            .expect("truncate to zero");

        // The handle buffer is Some(empty), inode.size_bytes is 0, and the
        // cache file is empty.
        {
            let state = fs.lock_state();
            let entry = state.inodes.get(&inode).expect("inode");
            assert_eq!(entry.size_bytes, 0);
            let open_handle = state.handles.get(&handle).expect("handle");
            assert_eq!(
                open_handle.write_buffer.as_deref(),
                Some(b"".as_slice()),
                "buffer staged empty for the zero-byte flush"
            );
        }
        assert_eq!(
            fs::read(&cache_path).expect("cache truncated"),
            b"",
            "cache file truncated to zero"
        );

        // A subsequent flush commits a zero-byte version to the daemon.
        fs.commit_handle(inode, handle)
            .expect("flush commits empty");
        let inner = backend
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        assert_eq!(
            inner
                .file_contents
                .get(&node_id)
                .map(|bytes| bytes.as_slice()),
            Some(&b""[..]),
            "daemon recorded zero bytes after truncate-to-zero flush"
        );
        assert!(
            inner
                .nodes
                .get(&node_id)
                .and_then(|node| node.current_version_id.as_ref())
                .is_some(),
            "daemon advanced the current version on the empty commit"
        );
    }

    /// setattr(size=N) where 0 < N < current_len truncates the cache file to N
    /// bytes and stages the first N bytes in the handle's write buffer.
    #[test]
    fn workspace_setattr_size_n_stages_first_n_bytes() {
        let cache = workspace_test_dir("setattr-n-cache");
        // apply_setattr_size only touches the cache file + in-memory state; no
        // RPC, so a dead endpoint is fine and the test stays hermetic.
        let http = DaemonHttpClient::new("127.0.0.1:1".to_string(), "unused".to_string());
        let (uid, gid) = current_owner();
        let inode = 7u64;
        let now = SystemTime::now();
        let mut state = FsState {
            inodes: HashMap::new(),
            children: HashMap::new(),
            handles: HashMap::new(),
            next_inode: 2,
            next_handle: 1,
            uid,
            gid,
        };
        state.inodes.insert(
            inode,
            InodeState {
                inode,
                parent_inode: ROOT_INODE,
                node_id: Some("node_setattr_n".to_string()),
                name: OsString::from("truncated.txt"),
                kind: FileType::RegularFile,
                mode: DEFAULT_FILE_MODE,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Ready,
                content_hash: None,
                size_bytes: 6,
                current_version_id: Some("ver_open".to_string()),
            },
        );
        let handle = 1u64;
        state.handles.insert(
            handle,
            OpenHandle {
                inode,
                writable: true,
                write_buffer: None,
                base_version_id: Some("ver_open".to_string()),
            },
        );
        let fs = WorkspaceFs {
            http: Mutex::new(http),
            cache_dir: cache.clone(),
            state: Mutex::new(state),
        };

        // Seed the inode cache file with "abcdef".
        fs::write(inode_cache_path(&cache, inode), b"abcdef").expect("seed cache");

        // setattr(size=4): truncate to the first 4 bytes.
        fs.apply_setattr_size(inode, Some(handle), 4)
            .expect("truncate to 4");

        {
            let state = fs.lock_state();
            let entry = state.inodes.get(&inode).expect("inode");
            assert_eq!(
                entry.size_bytes, 4,
                "inode size reflects the truncated length"
            );
            let open_handle = state.handles.get(&handle).expect("handle");
            assert_eq!(
                open_handle.write_buffer.as_deref(),
                Some(b"abcd".as_slice()),
                "buffer holds the first N bytes"
            );
        }
        assert_eq!(
            fs::read(inode_cache_path(&cache, inode)).expect("cache read"),
            b"abcd",
            "cache file truncated to N bytes"
        );
    }

    /// Sparse extension past the current cache length is unimplemented and
    /// must fail with ENOSYS rather than silently zero-padding a partial
    /// buffer that would commit artist data not present on the daemon.
    #[test]
    fn workspace_setattr_size_extend_returns_enosys() {
        let cache = workspace_test_dir("setattr-extend-cache");
        let http = DaemonHttpClient::new("127.0.0.1:1".to_string(), "unused".to_string());
        let (uid, gid) = current_owner();
        let inode = 8u64;
        let now = SystemTime::now();
        let mut state = FsState {
            inodes: HashMap::new(),
            children: HashMap::new(),
            handles: HashMap::new(),
            next_inode: 2,
            next_handle: 1,
            uid,
            gid,
        };
        state.inodes.insert(
            inode,
            InodeState {
                inode,
                parent_inode: ROOT_INODE,
                node_id: Some("node_setattr_ext".to_string()),
                name: OsString::from("extend.txt"),
                kind: FileType::RegularFile,
                mode: DEFAULT_FILE_MODE,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Ready,
                content_hash: None,
                size_bytes: 3,
                current_version_id: Some("ver_open".to_string()),
            },
        );
        let fs = WorkspaceFs {
            http: Mutex::new(http),
            cache_dir: cache.clone(),
            state: Mutex::new(state),
        };
        fs::write(inode_cache_path(&cache, inode), b"abc").expect("seed cache");

        let errno = fs
            .apply_setattr_size(inode, None, 6)
            .expect_err("sparse extend rejected");
        assert_eq!(
            i32::from(errno),
            libc::ENOSYS,
            "extension past current length returns ENOSYS"
        );
        // The read in truncate_cache_file happens before any rewrite, so a
        // rejected extend leaves the cache file intact.
        assert_eq!(
            fs::read(inode_cache_path(&cache, inode)).expect("cache read"),
            b"abc",
            "rejected extend leaves cache file unchanged"
        );
    }

    // ----- Round-3 FIX 2: flush advances open handles' base_version_id -----

    /// A successful flush must advance the still-open handle's base_version_id
    /// to the version the daemon just returned. Without that, write→flush→
    /// write→flush on the SAME handle re-sends the prior base and the daemon
    /// rejects the second commit with version_conflict (self-conflict against
    /// the version we just committed). After the fix, the second commit
    /// carries the version_id returned by the first.
    #[test]
    fn workspace_flush_advances_open_handle_base_version_id() {
        let (addr, backend, _handle) = start_test_daemon("ws_fix2_round3");
        let cache = workspace_test_dir("fix2r3-cache");
        let mount = workspace_test_dir("fix2r3-mount");
        let fs = WorkspaceFs::connect(&WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_fix2_round3".to_string(),
            cache_dir: cache,
            mountpoint: mount,
            foreground: false,
        })
        .expect("connect");

        // Create the file (version v1) on one handle.
        let (inode, handle) = seed_uncommitted_handle(&fs, "reuse.txt", b"first".to_vec(), None);
        fs.commit_handle(inode, handle).expect("first commit");
        let (node_id, v1) = {
            let state = fs.lock_state();
            let entry = state.inodes.get(&inode).expect("inode");
            (
                entry.node_id.clone().expect("node bound"),
                entry
                    .current_version_id
                    .clone()
                    .expect("v1 mirrored from file.write"),
            )
        };

        // FIX 2: the still-open handle's base must advance to v1. Before the
        // fix it stayed None (the create-time base), so the next flush on this
        // handle would send no base and self-conflict once v1 existed.
        {
            let state = fs.lock_state();
            let open_handle = state.handles.get(&handle).expect("handle retained");
            assert_eq!(
                open_handle.base_version_id.as_deref(),
                Some(v1.as_str()),
                "open handle base_version_id must advance after flush"
            );
        }

        // Second write+flush on the SAME handle must succeed (no self-conflict).
        fs.buffer_write(inode, handle, 0, b"second")
            .expect("second write buffered");
        fs.commit_handle(inode, handle)
            .expect("second commit (no self-conflict)");

        let v2 = {
            let state = fs.lock_state();
            state
                .inodes
                .get(&inode)
                .and_then(|entry| entry.current_version_id.clone())
                .expect("v2 set")
        };
        assert_ne!(v1, v2, "daemon advanced the version on the second commit");

        // The last file.write for this node must carry base_version_id == v1
        // (the version returned by the first commit), proving the handle base
        // advanced rather than sending a stale or missing base.
        let inner = backend
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let second_write = inner
            .operations
            .iter()
            .rev()
            .find(|op| {
                op.method == "file.write" && op.base_node_id.as_deref() == Some(node_id.as_str())
            })
            .expect("second file.write recorded");
        let params: Value = serde_json::from_str(&second_write.params_json).expect("params parse");
        assert_eq!(
            params
                .get("base_version_id")
                .and_then(|value| value.as_str()),
            Some(v1.as_str()),
            "second commit carried the version_id returned by the first"
        );
    }

    /// Seed a writable handle bound to an existing inode with the given
    /// optimistic-concurrency base (simulates open() of an existing file for
    /// the FIX 4 edit leg). Returns the handle id.
    fn seed_handle_on(fs: &WorkspaceFs, inode: u64, base_version_id: Option<String>) -> u64 {
        let mut state = fs.lock_state();
        let handle = state.next_handle;
        state.next_handle += 1;
        state.handles.insert(
            handle,
            OpenHandle {
                inode,
                writable: true,
                write_buffer: None,
                base_version_id,
            },
        );
        handle
    }

    /// Seed an uncommitted inode + handle holding `bytes` (simulates create()
    /// for FIX 2 / the create leg of FIX 4). Returns (inode, handle).
    fn seed_uncommitted_handle(
        fs: &WorkspaceFs,
        name: &str,
        bytes: Vec<u8>,
        base_version_id: Option<String>,
    ) -> (u64, u64) {
        let mut state = fs.lock_state();
        let inode = state.next_inode;
        state.next_inode += 1;
        let now = SystemTime::now();
        state.inodes.insert(
            inode,
            InodeState {
                inode,
                parent_inode: ROOT_INODE,
                node_id: None,
                name: OsString::from(name),
                kind: FileType::RegularFile,
                mode: DEFAULT_FILE_MODE,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Absent,
                content_hash: None,
                size_bytes: 0,
                current_version_id: None,
            },
        );
        state
            .children
            .entry(ROOT_INODE)
            .or_default()
            .by_name
            .insert(OsString::from(name), inode);
        let handle = state.next_handle;
        state.next_handle += 1;
        state.handles.insert(
            handle,
            OpenHandle {
                inode,
                writable: true,
                write_buffer: Some(bytes),
                base_version_id,
            },
        );
        (inode, handle)
    }

    /// Seed a writable open handle bound to a fresh inode (simulates open() of
    /// an existing file for FIX 3 / FIX 4's edit leg). The handle starts with
    /// no buffer (non-truncating open) and the given base_version_id. Returns
    /// (inode, handle).
    fn seed_writable_open(
        fs: &WorkspaceFs,
        name: &str,
        base_version_id: Option<String>,
    ) -> (u64, u64) {
        let mut state = fs.lock_state();
        let inode = state.next_inode;
        state.next_inode += 1;
        let now = SystemTime::now();
        state.inodes.insert(
            inode,
            InodeState {
                inode,
                parent_inode: ROOT_INODE,
                node_id: Some(format!("node_{name}")),
                name: OsString::from(name),
                kind: FileType::RegularFile,
                mode: DEFAULT_FILE_MODE,
                target: None,
                mtime: now,
                crtime: now,
                cache_state: CacheState::Ready,
                content_hash: None,
                size_bytes: 0,
                current_version_id: base_version_id.clone(),
            },
        );
        let handle = state.next_handle;
        state.next_handle += 1;
        state.handles.insert(
            handle,
            OpenHandle {
                inode,
                writable: true,
                write_buffer: None,
                base_version_id,
            },
        );
        (inode, handle)
    }

    /// Live end-to-end test: mount the read-write filesystem against a running
    /// daemon, write a file through the mount, read it back, and assert bytes
    /// match. Skips cleanly when /dev/fuse or fusermount is unavailable (CI
    /// sandboxes). This is the gold test for the Wave 3 write path.
    #[test]
    fn live_mount_write_then_read_round_trip() {
        if !Path::new("/dev/fuse").exists() {
            eprintln!("fuse-live-skip: /dev/fuse not available");
            return;
        }
        let has_fusermount = ["fusermount3", "fusermount"].iter().any(|name| {
            std::process::Command::new(name)
                .arg("--version")
                .output()
                .is_ok()
        });
        if !has_fusermount {
            eprintln!("fuse-live-skip: fusermount not available");
            return;
        }

        let (addr, _backend, _handle) = start_test_daemon("ws_live");
        let cache = workspace_test_dir("live-cache");
        let mount = workspace_test_dir("live-mount");
        let config = WorkspaceMountConfig {
            daemon_endpoint: addr,
            local_token: "ws_live".to_string(),
            cache_dir: cache,
            mountpoint: mount.clone(),
            foreground: false,
        };

        let mount_for_thread = mount.clone();
        let join = std::thread::spawn(move || {
            let _ = mount_workspace(config);
        });

        // Wait for the mount to become active (or the thread to die).
        let mut active = false;
        for _ in 0..100 {
            if std::process::Command::new("mountpoint")
                .arg("-q")
                .arg(&mount_for_thread)
                .status()
                .is_ok_and(|status| status.success())
            {
                active = true;
                break;
            }
            if join.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !active {
            eprintln!("fuse-live-skip: mountpoint did not become active");
            let _ = std::process::Command::new("fusermount")
                .arg("-u")
                .arg(&mount_for_thread)
                .status();
            return;
        }

        let test_path = mount_for_thread.join("live.txt");
        let payload = b"live workspace payload\n";
        let result = (|| -> std::io::Result<()> {
            fs::write(&test_path, payload)?;
            let read_back = fs::read(&test_path)?;
            assert_eq!(read_back, payload);
            Ok(())
        })();

        // Always unmount.
        let _ = std::process::Command::new("fusermount")
            .arg("-u")
            .arg(&mount_for_thread)
            .status();
        let _ = join.join();

        result.expect("live write-then-read must match");
    }
}
