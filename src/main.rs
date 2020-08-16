use std::fmt::{Display, Formatter};
use clap::{App, Arg, SubCommand, AppSettings};
use serde_json::Value;
use md5::Context;
use openssl::symm::Cipher;
use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::Write;
use indicatif::ProgressBar;

extern crate ureq;
extern crate clap;

fn main() -> Result<(), PageError> {
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
        for e in list.iter() {
            println!("Downloading {}", e.0.as_str());
            download(e.1.as_str(), PathBuf::from(e.0.as_str()).as_path())?;
        }
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
    let r = ureq::get(url)
        .set("x-access-token", "1rj2vRtegS8Y60B3w3qNZm5T2Q0TN2NR")
        .timeout_connect(5000)
        .call();
    if r.ok() {
        r.into_json().map_err(|v| PageError::IoError(v))
    } else {
        Err(PageError::PageResponseError(r.status()))
    }
}

fn download(url: &str, file: &Path) -> Result<(), PageError> {
    let res = ureq::get(url)
        .set("TE", "Trailers")
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; rv:68.0) Gecko/20100101 Firefox/68.0")
        .set("Referer", "https://twist.moe/")
        .timeout_connect(5000)
        .call();
    if !res.ok() {
        return Err(PageError::PageResponseError(res.status()))
    }
    let mut f = File::create(file).map_err(|e| PageError::IoError(e))?;
    std::io::copy(&mut res.into_reader(), &mut f).map_err(|e| PageError::IoError(e))?;
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