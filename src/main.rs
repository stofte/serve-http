use std::{
    env::{current_dir, set_current_dir},
    io::{prelude::*, BufReader, BufWriter},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    fs::File,
    sync::Arc,
    thread
};
use clap::{Parser};
use colored::*;
use dunce::canonicalize;
use native_tls::{Identity, TlsAcceptor, TlsStream};

const GET_VERB: &str = "GET ";
const HTTP_VER: &str = " HTTP/1.1";

#[derive(Parser, Debug)]
#[command(about="Basic utility for serving up a directory via HTTP", author, version = None, long_about = None)]
struct Args {
    /// Server port
    #[arg(short, long, default_value_t = 8080)]
    port: u16,

    /// Network interface to bind
    #[arg(short, long, default_value = "localhost")]
    bind: String,

    /// TLS certificate. Can be either a file path to a PFX file, 
    /// or a path to a locally installed certificate.
    #[arg(short, long)]
    tls_certificate: Option<String>,

    /// Server base directory. Defaults to the current directory if not set.
    wwwroot: Option<std::path::PathBuf>,
}

enum LogCategory {
    Info,
    Warning,
    Error,
}

fn main() {
    let args = Args::parse();

    let is_pfx_file = match args.tls_certificate {
        Some(path) => match std::fs::metadata(&path) {
            Ok(_) => true,
            Err(_) => false
        },
        None => false
    };


    let mut file = File::open("certs\\mypfx1.pfx").unwrap();
    let mut identity = vec![];
    file.read_to_end(&mut identity).unwrap();
    let identity = Identity::from_pkcs12(&identity, "foobar").unwrap();

    match &args.wwwroot {
        Some(p) => if !set_current_dir(&p).is_ok() {
            log(LogCategory::Warning, &format!(
                "Failed to set \"{}\" as base directory. Using \"{}\" instead.", 
                p.to_string_lossy(),
                current_dir().unwrap().to_string_lossy()
            ));
        },
        None => ()
    };

    let base_dir = current_dir().expect("Failed to get current dir");
    let bind_addr = [args.bind.clone(), args.port.to_string()].join(":");
    log(LogCategory::Info, &format!("Serving \"{}\" @ {}{}", base_dir.to_string_lossy(), "http://", bind_addr));

    let acceptor = TlsAcceptor::new(identity).unwrap();
    let acceptor = Arc::new(acceptor);
    let listener = TcpListener::bind(bind_addr).unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if true {
                    handle_connection(stream);
                } else {
                    let acceptor = acceptor.clone();
                    thread::spawn(move || {
                        let mut stream = acceptor.accept(stream).unwrap();
                        handle_connection(stream);
                    });
                }
            }
            Err(_) => { /* connection failed */ }
        }
    }
}

fn handle_connection(mut stream: impl Read + Write + Unpin) {
    let buf_reader = BufReader::new(&mut stream);
    if let Some(Ok(line)) = buf_reader.lines().nth(0) {
        if let Some(path) = translate_path(&line) {
            let mut writer = BufWriter::new(stream);
            let mut file_size = 0;
            let mut norm_path = path.to_string_lossy();
            let mut response_status: String = String::from("");
            let file_ok = match std::fs::metadata(&path) {
                Ok(metadata) => {
                    if metadata.is_file() {
                        file_size = metadata.len();
                        norm_path = String::from(canonicalize(&path).expect("Failed to canonicalize path").to_string_lossy()).into();
                        true
                    } else {
                        log(LogCategory::Info, &format!(
                            "Path is not a file. is_dir={}, is_symlink={}",
                            metadata.is_dir(),
                            metadata.is_symlink()
                        ));
                        false
                    }
                },
                Err(err) => {
                    log(LogCategory::Info, &format!("Failed to read metadata: {}", err));
                    false
                }
            };
            match file_ok {
                true => {
                    let f_wrapped = std::fs::File::open(&path);
                    if f_wrapped.is_ok() {
                        // File could be opened
                        let f = f_wrapped.unwrap();
                        let mut br = BufReader::new(f);
                        let lines = [
                            "HTTP/1.1 200 OK",
                            "Cache-Control: no-store",
                            &format!("Content-Length: {}\n\n", file_size)
                        ].join("\n");
                        if writer.write_all(lines.as_bytes()).is_ok() {
                            // All headers written, try to write file
                            if std::io::copy(&mut br, &mut writer).is_ok() {
                                response_status = format!("{} ({} bytes)", &norm_path, file_size);
                            } else {
                                log(LogCategory::Warning, &format!("Failed to write to file after headers"));
                            }
                        } else {
                            log(LogCategory::Info, &format!("Failed to write to response"));
                        }
                        writer.write_all(lines.as_bytes()).expect("Could not write");
                    } else {
                        if !writer.write_all("HTTP/1.1 500 Internal Server Error\n".as_bytes()).is_ok() {
                            log(LogCategory::Info, &format!("Failed to write to response"));
                        }
                    }
                },
                false => {
                    writer.write_all("HTTP/1.1 404 Not Found\n".as_bytes()).expect("Could not write");
                    response_status = String::from("404 Not Found");
                }
            }
            log(LogCategory::Info, &format!("Request {} => {}", &line[0..(line.len() - HTTP_VER.len())], response_status));
        }
    }
}

fn translate_path(line: &str) -> Option<PathBuf> {
    if line.starts_with(GET_VERB) && line.ends_with(HTTP_VER) {
        // Remove verb + HTTP version
        let mut path = String::from(&line[GET_VERB.len()..]);
        path.truncate(path.len() - HTTP_VER.len());

        // Format into a URL, so we can use parsing from std lib
        let dummyurl = String::from("http://localhost") + &path;
        let cur_dir = current_dir().expect("no path?");

        return match url::Url::parse(&dummyurl) {
            Ok(url) => {
                Some(std::path::Path::new(&cur_dir)
                    .join(".".to_owned() + std::path::MAIN_SEPARATOR_STR)
                    .join(".".to_owned() + &url.path().replace("/", "\\")))
            },
            Err(_) => None
        }
    }
    None
}

fn log(category: LogCategory, text: &str) {
    use chrono::prelude::*;

    let cat = match category {
        LogCategory::Info => "[INF]".white(),
        LogCategory::Warning => "[WRN]".yellow(),
        LogCategory::Error => "[ERR]".red()
    };
    let local: DateTime<Local> = Local::now();
    println!("{} {} {}", local.format("%T%.3f"), cat, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_translate_paths() -> () {
        let result = translate_path(&"GET /foo.txt HTTP/1.1");
        let pb = std::env::current_dir().expect("Could not get current_dir").join("foo.txt");
        assert_eq!(result, Some(pb));
    }
}
