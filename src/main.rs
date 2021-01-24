use std::fmt::{Display, Formatter};
use clap::{App, Arg, SubCommand, AppSettings};
use serde_json::Value;
use md5::Context;
use openssl::symm::Cipher;
use std::path::{Path, PathBuf};
use indicatif::{MultiProgress, ProgressStyle};
use reqwest::{Client, Response};
use std::time::Duration;
use std::error::Error;
use futures::AsyncWriteExt;
use bytes::Buf;
use reqwest::header::{USER_AGENT, REFERER};
use pbr::{MultiBar, Units, Pipe, ProgressBar};
use std::io::Stdout;
use std::thread::spawn;

extern crate clap;
extern crate tokio;

#[tokio::main]
async fn main() -> Result<(), TwistError> {
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
        let list = TwistPort::new()?.get_show_downloads(show).await?;
        for e in list.iter() {
            println!("\"{}\": {}", e.0, e.1);
        }
    } else if let Some(submatch) = a.subcommand_matches("download") {
        let port = TwistPort::new()?;
        let show = submatch.value_of("SHOW").unwrap();
        let list = port.get_show_downloads(show).await?;
        let m_bar = MultiBar::new();
        let mut show_bar = m_bar.create_bar(list.len() as u64);
        let mut down_bar = m_bar.create_bar(0);
        down_bar.set_units(Units::Bytes);
        spawn(move || m_bar.listen());
        for e in list.iter() {
            port.download_file(e.1.as_str(), PathBuf::from(e.0.as_str()).as_path(), &mut down_bar).await?;
            show_bar.inc();
        }
        down_bar.finish();
        show_bar.finish();
    } else if let Some(submatch) = a.subcommand_matches("size") {
        let port = TwistPort::new()?;
        let show = submatch.value_of("SHOW").unwrap();
        let list = port.get_show_downloads(show).await?;
        let mut size_acc = 0;
        let mut bar = ProgressBar::new(list.len() as u64);
        for e in list.iter()
            .map(|v| v.1.as_str())
        {
            bar.inc();
            size_acc += port.get_download_size(e).await?;
        }
        bar.finish();
        println!("Total: {}", size_to_string(size_acc))
    } else if a.subcommand_matches("list").is_some() {
        let list = TwistPort::new()?.list_shows().await?;
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

fn decrypt_source(s: &str) -> Result<String, TwistError> {
    // Decryption based on https://github.com/vn-ki/anime-downloader
    let dec = base64::decode(s).map_err(|_| TwistError::ParseError(String::from("invalid base64 source")))?;
    if dec.len() < 16 || !dec.as_slice().starts_with(b"Salted__") {
        return Err(TwistError::ParseError(String::from("invalid source format")));
    }
    const PASSPHRASE: &[u8] = b"267041df55ca2b36f2e322d05ee2c9cf";
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
    ).map_err(|_| TwistError::ParseError(String::from("decrypt fail")))?;
    let unquoted = std::str::from_utf8(decrypted.as_slice())
        .map_err(|_| TwistError::ParseError(String::from("decrypt encoding fail")))?;
    Ok(String::from(unquoted))
}

struct TwistPort {
    client: Client
}

#[derive(Debug)]
enum TwistError {
    AccessError(reqwest::Error),
    ParseError(String),
    IOError(std::io::Error)
}

impl Display for TwistError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TwistError::AccessError(e) => f.write_fmt(format_args!("AccessError: {}", e)),
            TwistError::ParseError(e) => f.write_fmt(format_args!("ParseError: {}", e)),
            TwistError::IOError(e) => f.write_fmt(format_args!("IOError: {}", e))
        }
    }
}

impl Error for TwistError {
}

impl TwistPort {
    fn new() -> Result<Self, TwistError> {
        Ok(TwistPort {
            client: Client::builder().referer(false).build().map_err(|e| TwistError::AccessError(e))?
        })
    }

    async fn raw_api_request(&self, url: &str) -> Result<Value, TwistError> {
        async fn inner_no_retry(port: &TwistPort, url: &str) -> Result<Value, TwistError> {
            Ok(port.client.get(url)
                .header("x-access-token", "0df14814b9e590a1f26d3071a4ed7974")
                .timeout(Duration::from_secs(10))
                .send().await
                .and_then(|v| v.error_for_status())
                .map_err(|e| TwistError::AccessError(e))?
                .json::<Value>().await.map_err(|e| TwistError::AccessError(e))?)
        }
        let mut i = 0;
        loop {
            match inner_no_retry(self, url).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    i += 1;
                    eprintln!("[WARN] retry {} of 10 failed", i);
                    if i == 10 {
                        return Err(e)
                    }
                }
            }
        }
    }

    async fn download_file(&self, url: &str, file: &Path, bar: &mut ProgressBar<Pipe>) -> Result<(), TwistError> {
        async fn try_send(port: &TwistPort, url: &str) -> reqwest::Result<Response> {
            let mut i = 0;
            loop {
                let v = port.client.get(url)
                    .header(USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; rv:78.0) Gecko/20100101 Firefox/78.0")
                    .header(REFERER, "https://twist.moe/")
                    .send().await;
                match v {
                    Ok(res) => {
                        return res.error_for_status()
                    },
                    Err(e) => {
                        println!("FAIL");
                        i += 1;
                        if i == 10 {
                            return Err(e)
                        }
                    }
                }
            }
        }
        bar.set(0);
        let mut res = try_send(self, url).await.map_err(|e| TwistError::AccessError(e))?;
        match res.headers().get("Content-Length")
            .and_then(|s| s.to_str().ok())
            .and_then(|s| s.parse().ok()) {
            Some(v) => {
                bar.total = v;
            },
            None => {
                bar.total = 0;
            }
        };
        let mut f = async_std::fs::File::create(file).await.map_err(|e| TwistError::IOError(e))?;
        while let Some(ch) = res.chunk().await.map_err(|e| TwistError::AccessError(e))? {
            f.write_all(ch.chunk()).await.map_err(|e| TwistError::IOError(e))?;
            bar.add(ch.len() as u64);
        }
        f.flush().await.map_err(|e| TwistError::IOError(e))?;
        Ok(())
    }

    async fn get_download_size(&self, url: &str) -> Result<usize, TwistError> {
        async fn try_send(port: &TwistPort, url: &str) -> reqwest::Result<Response> {
            let mut i = 0;
            loop {
                let v = port.client.head(url)
                    .header(USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; rv:78.0) Gecko/20100101 Firefox/78.0")
                    .header(REFERER, "https://twist.moe/")
                    .send().await;
                match v {
                    Ok(res) => {
                        return res.error_for_status()
                    },
                    Err(e) => {
                        i += 1;
                        if i == 10 {
                            return Err(e)
                        }
                    }
                }
            }
        }
        let res = try_send(self, url).await.map_err(|e| TwistError::AccessError(e))?;
        let size_str = res.headers().get("Content-Length").and_then(|v| v.to_str().ok())
            .ok_or(TwistError::ParseError(String::from("no content length")))?;
        let size_num: usize = size_str
            .parse()
            .map_err(|_| TwistError::ParseError(String::from("invalid content length")))?;
        Ok(size_num)
    }

    async fn list_shows(&self) -> Result<Vec<(String, String)>, TwistError> {
        let data = self.raw_api_request("https://twist.moe/api/anime").await?;
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
        res.ok_or(TwistError::ParseError(String::from("failed to parse anime list json")))
    }

    async fn get_show_downloads(&self, url: &str) -> Result<Vec<(String, String)>, TwistError> {
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
        let data = self.raw_api_request(url.as_str()).await?;

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
        })().ok_or(TwistError::ParseError(String::from("failed to parse anime source json")))
    }
}