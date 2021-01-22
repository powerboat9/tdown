use std::fmt::{Display, Formatter};
use clap::{App, Arg, SubCommand, AppSettings};
use serde_json::Value;
use md5::Context;
use openssl::symm::Cipher;
use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Write, Read};
use indicatif::{ProgressBar, MultiProgress, ProgressStyle};
use std::thread::spawn;
use fuse::{mount, Filesystem, Request, ReplyEntry, FileAttr, ReplyAttr, FileType, ReplyDirectory};
use std::ffi::{OsStr, CStr, OsString, CString};
use std::collections::HashMap;
use time::{Timespec, Duration};
use std::sync::mpsc::RecvTimeoutError::Timeout;
use chrono::NaiveDateTime;
use std::str::FromStr;
use chrono::format::{Parsed, Item, Numeric, Pad};
use std::cell::Cell;
use std::rc::Rc;
use bumpalo::Bump;
use std::os::raw::{c_char, c_int, c_void};

extern crate ureq;
extern crate clap;
extern crate time;
#[macro_use]
extern crate lazy_static;

//mod file_system;
#[allow(non_upper_case_globals)]
#[allow(non_camel_case_types)]
#[allow(non_snake_case)]
#[allow(dead_code)]
mod fuse3_sys;

use fuse3_sys::{stat, fuse_file_info};
use crate::fuse3_sys::{size_t, fuse_operations, timespec, off_t, fuse_fill_dir_t, fuse_readdir_flags};
use std::env::args;
use std::sync::{Once, Mutex};

struct AnimeStore {
    anime: Mutex<HashMap<String, Option<Anime>>>
}

impl AnimeStore {
    fn new() -> Self {
        AnimeStore {
            anime: Mutex::new(get_show_list_new().unwrap().into_iter().map(|v| (v.slug, None)).collect())
        }
    }

    fn list_anime(&self) -> Vec<String> {
        let r = self.anime.lock().unwrap();
        r.keys().map(|v| v.clone()).collect()
    }

    fn get_anime(&self, slug: &str) -> Anime {
        let mut r = self.anime.lock().unwrap();
        match r.entry(String::from(slug)).or_insert_with(|| {
            Some(Anime {
                episodes: get_show_downloads_new(slug).unwrap().into_iter().map(|(v, _)| v).collect()
            })
        }) {
            None => unreachable!(),
            Some(v) => v.clone()
        }
    }
}

lazy_static! {
    static ref ANIME_LIST: AnimeStore = AnimeStore::new();
}

#[derive(Clone)]
struct Anime {
    episodes: Vec<String>
}

enum FileRef {
    Root,
    AnimeDir(String),
    VideoFile(String, String)
}

impl FileRef {
    fn is_dir(&self) -> bool {
        match self {
            FileRef::Root => true,
            FileRef::AnimeDir(_) => true,
            FileRef::VideoFile(_, _) => false
        }
    }
}

fn parse_file_ref(filename: *const c_char) -> Option<FileRef> {
    let filename = unsafe {
        CStr::from_ptr(filename).to_str().ok()?
    };
    if filename == "/" {
        Some(FileRef::Root)
    } else {
        let mut f_ref = FileRef::Root;
        let mut sub_iter = filename.split('/').skip(1);
        let anime = sub_iter.next().unwrap();
        if let Some(episode) = sub_iter.next() {
            if sub_iter.next().is_some() {
                None
            } else {
                Some(FileRef::VideoFile(String::from(anime), String::from(episode)))
            }
        } else {
            Some(FileRef::AnimeDir(String::from(anime)))
        }
    }
}

extern "C" fn callback_getattr(filename: *const c_char, stat: *mut stat, info: *mut fuse_file_info) -> c_int {
    let file_ref = match parse_file_ref(filename) {
        Some(v) => v,
        None => return -(fuse3_sys::ENOENT as c_int)
    };
    let stat = unsafe {
        &mut *stat
    };
    stat.st_dev = 0;
    stat.st_ino = 0;
    if file_ref.is_dir() {
        stat.st_mode = fuse3_sys::S_IFDIR | 0o444;
    } else {
        stat.st_mode = fuse3_sys::S_IFREG | 0o444;
    }
    stat.st_nlink = 0;
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_size = 4096;
    stat.st_blksize = 512;
    stat.st_blocks = 8;
    stat.st_atim = timespec {
        tv_sec: 0,
        tv_nsec: 0
    };
    stat.st_mtim = timespec {
        tv_sec: 0,
        tv_nsec: 0
    };
    stat.st_ctim = timespec {
        tv_sec: 0,
        tv_nsec: 0
    };
    0
}

extern "C" fn callback_open(filename: *const c_char, info: *mut fuse_file_info) -> c_int {
    0
}

extern "C" fn callback_read(filename: *const c_char, buf: *mut c_char, buf_len: size_t, off: off_t, info: *mut fuse_file_info) -> c_int {
    let (anime, episode) = match parse_file_ref(filename) {
        None => return -(fuse3_sys::ENOENT as c_int),
        Some(FileRef::Root) | Some(FileRef::AnimeDir(_)) => return -(fuse3_sys::EISDIR as c_int),
        Some(FileRef::VideoFile(anime, episode)) => (anime, episode)
    };
    let buf = unsafe {
        std::slice::from_raw_parts_mut(buf, buf_len as usize)
    };
    let data = format!("ANIME: {}\nEPISODE: {}\n", anime, episode);
    if (off >= (data.len() as off_t)) || (off < 0) {
        return 0
    } else {
        let mut ret = 0;
        while ((ret as usize) < (data.len() - (off as usize))) && ((ret as usize) < (buf_len as usize)) {
            buf[ret as usize] = data.as_bytes()[(off + (ret as off_t)) as usize] as c_char;
            ret += 1;
        }
        ret
    }
}

extern "C" fn callback_readdir(filename: *const c_char, buf: *mut c_void, filler: fuse_fill_dir_t, off: off_t, info: *mut fuse_file_info, flags: fuse_readdir_flags) -> c_int {
    let filler = |name: &str| {
        let c_name = CString::new(name).unwrap();
        unsafe {
            (filler.unwrap())(buf, c_name.as_ptr(), 0 as *const stat, 0, 0)
        }
    };
    let file_ref = match parse_file_ref(filename) {
        Some(v) => v,
        None => return -(fuse3_sys::ENOENT as c_int)
    };
    match file_ref {
        FileRef::Root => {
            (filler)(".");
            (filler)("..");
            for anime in ANIME_LIST.list_anime() {
                (filler)(anime.as_str());
            }
            0
        }
        FileRef::AnimeDir(s) => {
            let anime = match Some(ANIME_LIST.get_anime(s.as_str())) {
                None => return -(fuse3_sys::ENOENT as c_int),
                Some(v) => v.episodes.into_iter()
            };
            (filler)(".");
            (filler)("..");
            for ep in anime {
                (filler)(ep.as_str());
            }
            0
        }
        FileRef::VideoFile(_, _) => {
            return -(fuse3_sys::ENOTDIR as c_int)
        }
    }
}

fn fuse_main(ops: &fuse_operations, private_data: *mut c_void) -> c_int {
    let mut args: Vec<_> = args().into_iter().collect();
    let mut argv: Vec<_> = args.iter_mut().map(|v| v.as_mut_ptr() as *mut c_char).collect();
    unsafe {
        fuse3_sys::fuse_main_real(argv.len() as c_int, argv.as_mut_ptr(), ops as *const fuse_operations, std::mem::size_of::<fuse3_sys::fuse_operations>() as size_t, private_data)
    }
}

fn main() -> Result<(), PageError> {
    let ops = fuse_operations {
        getattr: Some(callback_getattr),
        readlink: None,
        mknod: None,
        mkdir: None,
        unlink: None,
        rmdir: None,
        symlink: None,
        rename: None,
        link: None,
        chmod: None,
        chown: None,
        truncate: None,
        open: Some(callback_open),
        read: Some(callback_read),
        write: None,
        statfs: None,
        flush: None,
        release: None,
        fsync: None,
        setxattr: None,
        getxattr: None,
        listxattr: None,
        removexattr: None,
        opendir: None,
        readdir: Some(callback_readdir),
        releasedir: None,
        fsyncdir: None,
        init: None,
        destroy: None,
        access: None,
        create: None,
        lock: None,
        utimens: None,
        bmap: None,
        ioctl: None,
        poll: None,
        write_buf: None,
        read_buf: None,
        flock: None,
        fallocate: None,
        copy_file_range: None,
        lseek: None
    };
    fuse_main(&ops, 0 as *mut c_void);
    /*
    let a = App::new("Twist.moe Downloader")
        .version("1.0")
        .author("powerboat9")
        .about("Downloads shows from twist.moe")
        .subcommand(
            SubCommand::with_name("get-links")
                .about("Gets a list of download links from a show")
                .arg(
                    Arg::with_name("SHOW")
                        .help("the show to get links for")
                        .required(true)
                        .index(1)
                )
        )
        .subcommand(
            SubCommand::with_name("download")
                .about("Downloads a show to the current directory")
                .arg(
                    Arg::with_name("SHOW")
                        .help("the show to download")
                        .required(true)
                        .index(1)
                )
        )
        .subcommand(
            SubCommand::with_name("size")
                .about("Gets the size of a show")
                .arg(
                    Arg::with_name("SHOW")
                        .help("the show to size")
                        .required(true)
                        .index(1)
                )
        )
        .subcommand(
            SubCommand::with_name("list")
                .about("List shows available on twist.moe")
        )
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .get_matches();
    if let Some(submatch) = a.subcommand_matches("get-links") {
        let show = submatch.value_of("SHOW").unwrap();
        let list = get_show_downloads(show)?;
        for e in list.iter() {
            println!("\"{}\": {}", e.0, e.1);
        }
    } else if let Some(submatch) = a.subcommand_matches("download") {
        let show = submatch.value_of("SHOW").unwrap();
        let list = get_show_downloads(show)?;
        let progress = MultiProgress::new();
        let show_bar = ProgressBar::new(list.len() as u64);
        progress.add(show_bar.clone());
        show_bar.tick();
        let file_bar = ProgressBar::new(1);
        progress.add(file_bar.clone());
        file_bar.tick();
        spawn(move || {
            for e in show_bar.wrap_iter(list.iter()) {
                download(e.1.as_str(), PathBuf::from(e.0.as_str()).as_path(), file_bar.clone()).unwrap();
            }
        });
        progress.join().unwrap();
    } else if let Some(submatch) = a.subcommand_matches("size") {
        let show = submatch.value_of("SHOW").unwrap();
        let list = get_show_downloads(show)?;
        let mut size_acc = 0;
        let bar = ProgressBar::new(list.len() as u64);
        for e in list.iter()
            .map(|v| v.1.as_str())
            .enumerate()
        {
            bar.set_position(e.0 as u64 + 1);
            size_acc += get_download_size(e.1)?;
        }
        bar.finish();
        println!("Total: {}", size_to_string(size_acc))
    } else if a.subcommand_matches("list").is_some() {
        let list = get_show_list()?;
        for e in list.iter() {
            println!("\"{}\": {}", e.0, e.1);
        }
    }
     */
    Ok(())
}

fn size_to_string(n: usize) -> String {
    let f = n as f64;
    let sizes = ["", " KiB", " MiB", " GiB", " TiB", " PiB"];
    let mut fsize = 1.;
    let mut size_idx = 0;
    loop {
        let r = f / fsize;
        if (r < 1024.) || (size_idx == (sizes.len() - 1)) {
            break format!("{}{}", r as u32, sizes[size_idx]);
        }
        fsize *= 1024.;
        size_idx += 1;
    }
}

#[derive(Debug)]
enum PageError {
    PageResponseError(u16),
    IoError(std::io::Error),
    ParseError(&'static str)
}

impl Display for PageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            PageError::PageResponseError(c) => f.write_fmt(format_args!("Response Error: {}", c)),
            PageError::IoError(e) => f.write_fmt(format_args!("IO Error: {}", e)),
            PageError::ParseError(s) => f.write_fmt(format_args!("Parsing Failure: {}", *s))
        }
    }
}

impl std::error::Error for PageError {
}

fn api_request(url: &str) -> Result<Value, PageError> {
    for i in 0..1 {
        let r = ureq::get(url)
            .set("x-access-token", /*"1rj2vRtegS8Y60B3w3qNZm5T2Q0TN2NR"*/"0df14814b9e590a1f26d3071a4ed7974")
            .set("Origin", "https://twist.moe")
            .timeout_connect(5000)
            .call();
        if !r.ok() {
            continue;
        }
        return r.into_json().map_err(|v| PageError::IoError(v))
    }
    Err(PageError::PageResponseError(0))
}

fn download(url: &str, file: &Path, bar: ProgressBar) -> Result<(), PageError> {
    let res = ureq::get(url)
        .set("TE", "Trailers")
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; rv:68.0) Gecko/20100101 Firefox/68.0")
        .set("Referer", "https://twist.moe/")
        .timeout_connect(5000)
        .call();
    if !res.ok() {
        return Err(PageError::PageResponseError(res.status()))
    }
    match res.header("Content-Length").and_then(|s| s.parse().ok()) {
        Some(v) => {
            bar.set_length(v);
            bar.set_style(ProgressStyle::default_bar().template("{wide_bar} {bytes}/{total_bytes}"))
        },
        None => {
            bar.set_length(!0);
            bar.set_style(ProgressStyle::default_spinner())
        }
    }
    bar.set_position(0);
    let mut f = File::create(file).map_err(|e| PageError::IoError(e))?;
    std::io::copy(&mut bar.wrap_read(res.into_reader()), &mut f).map_err(|e| PageError::IoError(e))?;
    f.flush().map_err(|e| PageError::IoError(e))?;
    Ok(())
}

fn get_download_size(url: &str) -> Result<usize, PageError> {
    let res = ureq::head(url)
        .set("TE", "Trailers")
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; rv:68.0) Gecko/20100101 Firefox/68.0")
        .set("Referer", "https://twist.moe/")
        .timeout_connect(5000)
        .call();
    if !res.ok() {
        return Err(PageError::PageResponseError(res.status()))
    }
    let size_str = res.header("Content-Length")
        .ok_or(PageError::ParseError("no content length"))?;
    let size_num: usize = size_str
        .parse()
        .map_err(|_| PageError::ParseError("invalid content length"))?;
    Ok(size_num)
}

fn get_show_downloads_new(stub: &str) -> Result<Vec<(String, String)>, PageError> {
    let url = format!("https://twist.moe/api/anime/{}/sources", stub);
    let data = api_request(url.as_str())?;

    (|| {
        let mut ls = Vec::new();
        for ent in data.as_array()? {
            let entv = ent.as_object()?;
            let ob_source = entv.get("source")?.as_str()?;
            let source = decrypt_source(ob_source).ok()?;
            let file_name = {
                match source.rfind('/') {
                    Some(idx) => String::from(&source.as_str()[(idx + 1)..]),
                    None => source.clone()
                }
            };
            let quoted_source = format!("https://twistcdn.bunny.sh{}", urlencoding::encode(source.as_str()).replace("%2F", "/"));
            ls.push((file_name, quoted_source));
        }
        Some(ls)
    })().ok_or(PageError::ParseError("failed to parse json"))
}

fn get_show_downloads(url: &str) -> Result<Vec<(String, String)>, PageError> {
    let stub = {
        let mut tmp = url;
        if tmp.ends_with('/') {
            tmp = &tmp[..(tmp.len() - 1)];
        }
        match tmp.rfind('/') {
            Some(idx) => &tmp[(idx + 1)..],
            None => tmp
        }
    };

    let url = format!("https://twist.moe/api/anime/{}/sources", stub);
    let data = api_request(url.as_str())?;

    (|| {
        let mut ls = Vec::new();
        for ent in data.as_array()? {
            let entv = ent.as_object()?;
            let ob_source = entv.get("source")?.as_str()?;
            let source = decrypt_source(ob_source).ok()?;
            let file_name = {
                match source.rfind('/') {
                    Some(idx) => String::from(&source.as_str()[(idx + 1)..]),
                    None => source.clone()
                }
            };
            let quoted_source = format!("https://twistcdn.bunny.sh{}", urlencoding::encode(source.as_str()).replace("%2F", "/"));
            ls.push((file_name, quoted_source));
        }
        Some(ls)
    })().ok_or(PageError::ParseError("failed to parse json"))
}

fn decrypt_source(s: &str) -> Result<String, PageError> {
    // Decryption based on https://github.com/vn-ki/anime-downloader
    let dec = base64::decode(s).map_err(|_| PageError::ParseError("invalid base64 source"))?;
    if dec.len() < 16 || !dec.as_slice().starts_with(b"Salted__") {
        return Err(PageError::ParseError("invalid source format"));
    }
    const PASSPHRASE: &[u8] = b"LXgIVP&PorO68Rq7dTx8N^lP!Fa5sGJ^*XK";
    let salt = &dec.as_slice()[8..16];
    // obtains key and iv
    fn bytes_to_key_iv(data: &[u8], salt: &[u8]) -> ([u8; 32], [u8; 16]) {
        fn hash_buffers(data: &[&[u8]]) -> [u8; 16] {
            let mut ctx = Context::new();
            for ent in data.iter().copied() {
                ctx.consume(ent);
            }
            ctx.compute().0
        }
        let key1 = hash_buffers(&[data, salt]);
        let key2 = hash_buffers(&[&key1, data, salt]);
        let key3 = hash_buffers(&[&key2, data, salt]);
        let mut combo_key = [0; 32];
        for i in 0..16 {
            combo_key[i] = key1[i];
            combo_key[16 + i] = key2[i];
        }
        (combo_key, key3)
    }
    let (key, iv) = bytes_to_key_iv(PASSPHRASE, salt);
    let decrypted = openssl::symm::decrypt(
        Cipher::aes_256_cbc(),
        &key,
        Some(&iv),
        dec.as_slice().split_at(16).1
    ).map_err(|_| PageError::ParseError("decrypt fail"))?;
    let unquoted = std::str::from_utf8(decrypted.as_slice())
        .map_err(|_| PageError::ParseError("decrypt encoding fail"))?;
    Ok(String::from(unquoted))
}

struct ShowListEntry {
    eng_title: Option<String>,
    jap_title: String,
    created: Timespec,
    updated: Timespec,
    slug: String
}

fn get_show_list_new() -> Result<Vec<ShowListEntry>, PageError> {
    let data = api_request("https://twist.moe/api/anime")?;
    let mut ls = Vec::new();
    let res: Option<_> = (|| {
        for ent in data.as_array()? {
            let ent_map = ent.as_object()?;
            let eng_title = ent_map.get("alt_title")?;
            let eng_title = if eng_title.is_null() {
                None
            } else {
                Some(String::from(eng_title.as_str()?))
            };
            ls.push(ShowListEntry {
                eng_title,
                jap_title: String::from(ent_map.get("title")?.as_str()?),
                created: parse_twist_time(/*ent_map.get("created_at")?.as_str()?*/"")?,
                updated: parse_twist_time(/*ent_map.get("updated_at")?.as_str()?*/"")?,
                slug: String::from(ent_map.get("slug")?.as_object()?.get("slug")?.as_str()?)
            });
        }
        Some(ls)
    })();
    res.ok_or(PageError::ParseError("failed to manage json"))
}

fn parse_twist_time(s: &str) -> Option<Timespec> {
    /*
    let mut parsed = Parsed::new();
    chrono::format::parse(&mut parsed, s, [
        Item::Numeric(Numeric::Year, Pad::Zero),
        Item::Literal("-"),
        Item::Numeric(Numeric::Month, Pad::Zero),
        Item::Literal("-"),
        Item::Numeric(Numeric::Day, Pad::Zero),
        Item::Space(""),
        Item::Numeric(Numeric::Hour, Pad::Zero),
        Item::Literal(":"),
        Item::Numeric(Numeric::Minute, Pad::Zero),
        Item::Literal(":"),
        Item::Numeric(Numeric::Second, Pad::Zero)
    ].iter()).ok()?;
    Some(Timespec::new(parsed.to_datetime().ok()?.timestamp(), 0))
     */
    Some(Timespec::new(0, 0))
}

fn get_show_list() -> Result<Vec<(String, String)>, PageError> {
    let data = api_request("https://twist.moe/api/anime")?;
    let mut ls = Vec::new();
    let res: Option<_> = (|| {
        for ent in data.as_array()? {
            let ent_map = ent.as_object()?;
            let name = String::from(ent_map.get("title")?.as_str()?);
            let slug = ent_map.get("slug")?.as_object()?.get("slug")?.as_str()?;
            ls.push((name, format!("https://twist.moe/a/{}", slug)));
        }
        Some(ls)
    })();
    res.ok_or(PageError::ParseError("failed to manage json"))
}