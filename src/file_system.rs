use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::Read;
use std::rc::Rc;

use bumpalo::Bump;
use fuse::{FileAttr, Filesystem, FileType, ReplyEntry, Request, ReplyAttr};
use libc::{EBUSY, ENOENT, ENOTDIR};
use owning_ref::{OwningHandle, OwningRef};
use time::Timespec;

use crate::{get_show_downloads_new, get_show_list_new, ShowListEntry};

struct BackedFS<'a> {
    bump: &'a Bump,
    inode_list: Vec<&'a dyn FUSEFile<'a>>,
}

impl<'a> BackedFS<'a> {
    fn new(bump: &'a Bump, root_dir: impl 'a + FUSEFile<'a>) -> Self {
        let mut ret = BackedFS {
            bump,
            inode_list: Vec::new(),
        };
        ret.register(root_dir);
        ret
    }

    fn register(&mut self, file: impl 'a + FUSEFile<'a>) -> &'a dyn FUSEFile<'a> {
        let f_ref = self.bump.alloc(file);
        f_ref.set_inode((self.inode_list.len() as u64) + 1);
        let f_ref = &*f_ref;
        self.inode_list.push(f_ref);
        f_ref
    }
}

trait FUSEFile<'a> {
    fn list(&'a self, fs: &'a mut BackedFS<'a>) -> Result<Vec<(String, u64)>, i32>;
    fn lookup(&'a self, fs: &'a mut BackedFS<'a>, file_name: &str) -> Result<u64, i32>;
    fn set_inode(&mut self, inode: u64);
    fn get_inode(&self) -> u64;
    fn get_size(&self) -> u64;
    fn get_file_type(&self) -> FileType;
    fn get_attr(&self) -> FileAttr {
        let size = self.get_size();
        FileAttr {
            ino: self.get_inode(),
            size,
            blocks: (size + 511) / 512,
            atime: Timespec::new(0, 0),
            mtime: Timespec::new(0, 0),
            ctime: Timespec::new(0, 0),
            crtime: Timespec::new(0, 0),
            kind: self.get_file_type(),
            perm: 0o444,
            nlink: 0,
            uid: 0,
            gid: 0,
            rdev: 0,
            flags: 0
        }
    }
}

struct TwistEpisodeFile {
    url: String,
    inode: u64,
}

impl<'a> FUSEFile<'a> for TwistEpisodeFile {
    fn list(&'a self, _fs: &'a mut BackedFS) -> Result<Vec<(String, u64)>, i32> {
        Err(ENOTDIR)
    }

    fn lookup(&'a self, _fs: &'a mut BackedFS, _file_name: &str) -> Result<u64, i32> {
        Err(ENOTDIR)
    }

    fn set_inode(&mut self, inode: u64) {
        self.inode = inode;
    }

    fn get_inode(&self) -> u64 {
        self.inode
    }

    fn get_size(&self) -> u64 {
        1024 * 1024 * 4
    }

    fn get_file_type(&self) -> FileType {
        FileType::RegularFile
    }
}

impl TwistEpisodeFile {
    fn new(url: String) -> Self {
        TwistEpisodeFile {
            url,
            inode: 0,
        }
    }
}

struct EpisodeTwistDir<'a> {
    cache: RefCell<Option<Vec<(String, &'a dyn FUSEFile<'a>)>>>,
    stub: &'a str,
    inode: u64,
}

impl<'a> FUSEFile<'a> for EpisodeTwistDir<'a> {
    fn list(&'a self, fs: &'a mut BackedFS<'a>) -> Result<Vec<(String, u64)>, i32> {
        match &mut *self.cache.borrow_mut() {
            cache @ None => {
                let mut ret = Vec::new();
                let mut new_cache = Vec::new();
                for down in get_show_downloads_new(self.stub).map_err(|_| EBUSY)?.into_iter() {
                    let vid_ref = fs.register(TwistEpisodeFile::new(down.1));
                    ret.push((down.0.clone(), vid_ref.get_inode()));
                    new_cache.push((down.0, vid_ref));
                }
                *cache = Some(new_cache);
                Ok(ret)
            }
            Some(ls) => {
                Ok(ls.iter().map(|v| (v.0.clone(), v.1.get_inode())).collect())
            }
        }
    }

    fn lookup(&'a self, fs: &'a mut BackedFS<'a>, file_name: &str) -> Result<u64, i32> {
        match &mut *self.cache.borrow_mut() {
            cache @ None => {
                let mut ret = Err(ENOENT);
                let mut new_cache = Vec::new();
                for down in get_show_downloads_new(self.stub).map_err(|_| EBUSY)?.into_iter() {
                    let vid_ref = fs.register(TwistEpisodeFile::new(down.1));
                    match &mut ret {
                        Ok(_) => {}
                        err @ Err(_) => {
                            if down.0.as_str() == file_name {
                                *err = Ok(vid_ref.get_inode())
                            }
                        }
                    }
                    new_cache.push((down.0, vid_ref));
                }
                *cache = Some(new_cache);
                ret
            }
            Some(ls) => {
                match ls.iter().filter(|v| v.0.as_str() == file_name).next() {
                    None => Err(ENOENT),
                    Some(v) => Ok(v.1.get_inode())
                }
            }
        }
    }

    fn set_inode(&mut self, inode: u64) {
        self.inode = inode;
    }

    fn get_inode(&self) -> u64 {
        self.inode
    }

    fn get_size(&self) -> u64 {
        4096
    }

    fn get_file_type(&self) -> FileType {
        FileType::Directory
    }
}

impl<'a> EpisodeTwistDir<'a> {
    fn new(stub: &'a str) -> Self {
        EpisodeTwistDir {
            cache: RefCell::new(None),
            stub,
            inode: 0,
        }
    }
}

struct RootTwistDir<'a> {
    cache: RefCell<Option<Vec<(String, &'a dyn FUSEFile<'a>)>>>,
    inode: u64,
}

impl<'a> FUSEFile<'a> for RootTwistDir<'a> {
    fn list(&'a self, fs: &'a mut BackedFS<'a>) -> Result<Vec<(String, u64)>, i32> {
        match &mut *self.cache.borrow_mut() {
            cache @ None => {
                let mut ret = Vec::new();
                let mut new_cache = Vec::new();
                for anime in get_show_list_new().map_err(|_| EBUSY)? {
                    let ShowListEntry { slug, .. } = anime;
                    let slug_ptr = &*fs.bump.alloc_str(slug.as_str());
                    let anime_ref = fs.register(EpisodeTwistDir::new(slug_ptr));
                    ret.push((slug.clone(), anime_ref.get_inode()));
                    new_cache.push((slug, anime_ref));
                }
                *cache = Some(new_cache);
                Ok(ret)
            }
            Some(ls) => {
                Ok(ls.iter().map(|v| (v.0.clone(), v.1.get_inode())).collect())
            }
        }
    }

    fn lookup(&self, fs: &'a mut BackedFS<'a>, file_name: &str) -> Result<u64, i32> {
        match &mut *self.cache.borrow_mut() {
            cache @ None => {
                let mut ret = Err(ENOENT);
                let mut new_cache = Vec::new();
                for anime in get_show_list_new().map_err(|_| EBUSY)? {
                    let ShowListEntry { slug, .. } = anime;
                    let slug_ptr = &*fs.bump.alloc_str(slug.as_str());
                    let anime_ref = fs.register(EpisodeTwistDir::new(slug_ptr));
                    match &mut ret {
                        Ok(_) => {}
                        err @ Err(_) => {
                            if slug.as_str() == file_name {
                                *err = Ok(anime_ref.get_inode())
                            }
                        }
                    }
                    new_cache.push((slug, anime_ref));
                }
                *cache = Some(new_cache);
                ret
            }
            Some(ls) => {
                match ls.iter().filter(|v| v.0.as_str() == file_name).next() {
                    None => Err(ENOENT),
                    Some(v) => Ok(v.1.get_inode())
                }
            }
        }
    }

    fn set_inode(&mut self, inode: u64) {
        self.inode = inode;
    }

    fn get_inode(&self) -> u64 {
        self.inode
    }

    fn get_size(&self) -> u64 {
        4096
    }

    fn get_file_type(&self) -> FileType {
        FileType::Directory
    }
}

impl<'a> BackedFS<'a> {
    fn get_inode(&self, ino: u64) -> Option<&'a dyn FUSEFile> {
        self.inode_list.get(ino.checked_sub(1)?)
    }
}

impl<'a> Filesystem for BackedFS<'a> {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let file_name = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        match self.get_inode(parent) {
            None => reply.error(ENOENT),
            Some(inode) => {
                match inode.lookup(self, file_name) {
                    Ok(o) => {
                        reply.entry(&Timespec::new(3, 0), &self.get_inode(o).unwrap().get_attr(), 0)
                    }
                    Err(_) => {

                    }
                }
            }
        }
        if parent == 0 {
            reply.error(EBUSY);
        } else {
            let idx = (parent - 1) as usize;
            match self.inode_list[idx].lookup(self, file_name) {
                Ok(o) => {
                    reply.entry()
                },
                Err(e) => {
                    reply.error(e)
                }
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {

    }
}