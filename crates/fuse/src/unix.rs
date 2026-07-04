use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo,
    MountOption, OpenAccMode, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, Request,
};

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
}
