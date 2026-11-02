// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Build tasks, in Rust so they run the same on every OS and in CI.
//!
//!   cargo xtask web [--dev]       build the browser version into dist/
//!   cargo xtask serve [--port N]  serve dist/ on http://127.0.0.1:8080

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

/// The largest wasm we accept, and Cloudflare's per-file limit with margin.
const WASM_BUDGET: u64 = 12 * 1024 * 1024;
const FILE_LIMIT: u64 = 24 * 1024 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("web") => web(args.iter().any(|a| a == "--dev")),
        Some("serve") => {
            let port = args
                .iter()
                .position(|a| a == "--port")
                .and_then(|i| args.get(i + 1))
                .and_then(|p| p.parse().ok())
                .unwrap_or(8080);
            serve(port)
        }
        _ => Err("usage: cargo xtask web [--dev] | serve [--port N]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn web(dev: bool) -> Result<(), String> {
    let root = root();
    let profile = if dev { "dev" } else { "web" };
    let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(&root)
        .args([
            "build",
            "-p",
            "splatstudy-web",
            "--target",
            "wasm32-unknown-unknown",
            "--profile",
            profile,
        ])
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err("wasm build failed".into());
    }

    let out_dir = if dev { "debug" } else { "web" };
    let wasm = root
        .join("target/wasm32-unknown-unknown")
        .join(out_dir)
        .join("splatstudy_web.wasm");
    let dist = root.join("dist");
    let _ = fs::remove_dir_all(&dist);
    fs::create_dir_all(&dist).map_err(|e| e.to_string())?;

    wasm_bindgen_cli_support::Bindgen::new()
        .input_path(&wasm)
        .web(true)
        .map_err(|e| e.to_string())?
        .debug(dev)
        .keep_debug(dev)
        .generate(&dist)
        .map_err(|e| format!("wasm-bindgen: {e}"))?;

    let bg = dist.join("splatstudy_web_bg.wasm");
    if !dev {
        wasm_opt(&bg);
    }
    for entry in fs::read_dir(root.join("web")).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        fs::copy(entry.path(), dist.join(entry.file_name())).map_err(|e| e.to_string())?;
    }
    // Stamp a content hash into index.html so caches never mix a new page
    // with an old wasm.
    let build = fnv1a(&fs::read(&bg).map_err(|e| e.to_string())?);
    let index = dist.join("index.html");
    let html = fs::read_to_string(&index).map_err(|e| e.to_string())?;
    fs::write(&index, html.replace("__BUILD__", &format!("{build:016x}")))
        .map_err(|e| e.to_string())?;
    report_sizes(&dist, &bg)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Shrinks the wasm with binaryen's wasm-opt when it is installed.
fn wasm_opt(wasm: &Path) {
    let local = root().join("tools/binaryen/bin/wasm-opt");
    let exe = if local.with_extension("exe").exists() || local.exists() {
        local.into_os_string()
    } else {
        "wasm-opt".into()
    };
    let result = Command::new(&exe)
        .arg(wasm)
        .args(["-Os", "-o"])
        .arg(wasm)
        .args([
            "--enable-bulk-memory",
            "--enable-nontrapping-float-to-int",
            "--enable-sign-ext",
            "--enable-mutable-globals",
            "--enable-reference-types",
            "--enable-multivalue",
        ])
        .status();
    match result {
        Ok(s) if s.success() => println!("wasm-opt: done"),
        Ok(_) => eprintln!("wasm-opt failed; keeping the unoptimised wasm"),
        Err(_) => eprintln!("wasm-opt not found; skipping (install binaryen into tools/binaryen)"),
    }
}

fn report_sizes(dist: &Path, wasm: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dist).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let size = entry.metadata().map_err(|e| e.to_string())?.len();
        println!(
            "{:>10.1} KiB  {}",
            size as f64 / 1024.0,
            entry.file_name().to_string_lossy()
        );
        if size > FILE_LIMIT {
            return Err(format!(
                "{} is over the per-file limit",
                entry.path().display()
            ));
        }
    }
    let size = fs::metadata(wasm).map_err(|e| e.to_string())?.len();
    if size > WASM_BUDGET {
        return Err(format!(
            "wasm is {size} bytes, over the {WASM_BUDGET} byte budget"
        ));
    }
    Ok(())
}

/// A minimal static server for dist/, with the MIME types browsers insist on.
fn serve(port: u16) -> Result<(), String> {
    let dist = root().join("dist");
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    println!("serving {} on http://127.0.0.1:{port}", dist.display());
    for stream in listener.incoming().flatten() {
        let _ = respond(stream, &dist);
    }
    Ok(())
}

fn respond(mut stream: TcpStream, dist: &Path) -> std::io::Result<()> {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    // Read the rest of the request: closing with unread input resets the
    // connection, and the browser then sees a truncated response.
    let mut header = String::new();
    while reader.read_line(&mut header)? > 2 {
        header.clear();
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let path = path
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let file = dist.join(path);
    let inside = file
        .canonicalize()
        .is_ok_and(|f| f.starts_with(dist.canonicalize().unwrap_or_default()));
    let (status, body, mime) = match fs::read(&file) {
        Ok(body) if inside => ("200 OK", body, mime(&file)),
        _ => ("404 Not Found", b"not found".to_vec(), "text/plain"),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}
