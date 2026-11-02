use anyhow::{Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Args, Debug, Clone)]
pub struct ServeArgs {
    /// Directory path to serve. Defaults to current directory (".").
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Port to listen on.
    #[arg(default_value = "8080")]
    pub port: u16,

    /// Host interface to bind to.
    #[arg(short = 'H', long, default_value = "0.0.0.0")]
    pub host: String,

    /// Single Page Application (SPA) mode: route not-found requests to index.html.
    #[arg(long)]
    pub spa: bool,
}

pub async fn run(args: ServeArgs) -> Result<()> {
    let root_dir = if args.path.is_relative() {
        std::env::current_dir()?.join(&args.path)
    } else {
        args.path.clone()
    };

    let canonical_root = match root_dir.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            anyhow::bail!("Directory to serve does not exist: {}", root_dir.display());
        }
    };

    let addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind server to {}", addr))?;

    let spa_label = if args.spa { " [SPA Mode]" } else { "" };
    println!(
        "\x1b[1;32m[orion serve]\x1b[0m Serving '{}' on http://{}{}",
        canonical_root.display(),
        addr,
        spa_label
    );
    println!("Press Ctrl+C to stop.");

    let shared_root = Arc::new(canonical_root);
    let is_spa = args.spa;

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let root = shared_root.clone();
                tokio::spawn(async move {
                    let _ = handle_http_client(stream, root, is_spa).await;
                });
            }
            Err(e) => {
                eprintln!("[orion serve] Connection error: {}", e);
            }
        }
    }
}

async fn handle_http_client(mut stream: TcpStream, root: Arc<PathBuf>, spa: bool) -> Result<()> {
    let mut buffer = [0u8; 4096];
    let n = stream.read(&mut buffer).await?;
    if n == 0 {
        return Ok(());
    }

    let request_str = String::from_utf8_lossy(&buffer[..n]);
    let first_line = request_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();

    let method = parts.next().unwrap_or("GET");
    let raw_uri = parts.next().unwrap_or("/");

    if method != "GET" && method != "HEAD" {
        let resp =
            "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        stream.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    // Strip query parameters
    let uri_path = raw_uri.split('?').next().unwrap_or("/");

    // URL decode basic path
    let decoded_path = urlencoding_decode(uri_path);
    let clean_rel = decoded_path.trim_start_matches('/');

    let target_path = if clean_rel.is_empty() {
        root.join("index.html")
    } else {
        root.join(clean_rel)
    };

    // Prevent directory traversal
    let safe_target = if let Ok(canonical) = target_path.canonicalize() {
        if canonical.starts_with(root.as_path()) {
            if canonical.is_dir() {
                let index = canonical.join("index.html");
                if index.is_file() {
                    Some(index)
                } else {
                    None
                }
            } else if canonical.is_file() {
                Some(canonical)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let (content, status, mime) = if let Some(ref path) = safe_target {
        match tokio::fs::read(path).await {
            Ok(bytes) => {
                let mime = guess_mime(path);
                (bytes, 200, mime)
            }
            Err(_) => (b"Internal Error".to_vec(), 500, "text/plain"),
        }
    } else if spa {
        // Fallback to index.html for Single Page Applications
        let index_path = root.join("index.html");
        match tokio::fs::read(&index_path).await {
            Ok(bytes) => (bytes, 200, "text/html; charset=utf-8"),
            Err(_) => (b"Not Found".to_vec(), 404, "text/plain"),
        }
    } else {
        (b"404 Not Found".to_vec(), 404, "text/plain")
    };

    let status_line = match status {
        200 => "HTTP/1.1 200 OK",
        404 => "HTTP/1.1 404 Not Found",
        500 => "HTTP/1.1 500 Internal Server Error",
        _ => "HTTP/1.1 400 Bad Request",
    };

    let header = format!(
        "{}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        status_line,
        mime,
        content.len()
    );

    stream.write_all(header.as_bytes()).await?;
    if method == "GET" {
        stream.write_all(&content).await?;
    }
    stream.flush().await?;

    Ok(())
}

fn guess_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn urlencoding_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                if let Ok(byte) = u8::from_str_radix(&format!("{}{}", c1, c2), 16) {
                    result.push(byte as char);
                    continue;
                }
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}
