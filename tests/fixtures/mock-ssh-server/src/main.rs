//! Headless mock for the Phase 6 §6.6 install-wizard tests.
//!
//! NOT a real SSH server. This binary is a minimal TCP listener that
//! accepts any incoming connection, reads a single newline-delimited
//! "install" command, and writes a JSON file to the inbox directory.
//!
//! Wire protocol (deliberately tiny — the install.rs tests speak this
//! directly, not via ssh2):
//!
//!   request  = `<collector_id>\n<arbitrary payload bytes>`
//!   response = `ok\n`
//!
//! One TCP connection == one install. The server writes one
//! `install-<seq>.json` to `<inbox>/`. Each JSON file is
//!
//!   ```json
//!   {"timestamp": "<iso8601-utc>", "collector_id": "...", "payload": "..."}
//!   ```
//!
//! The server accepts the binary's own `--port 0` (ephemeral) so the
//! Rust test process can grab a free port and point `apply_install_plan_localhost`
//! at it. SO_REUSEADDR is set so a previous server on the same port
//! doesn't hold it for TIME_WAIT after the test exits.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use clap::Parser;
use serde_json::json;

/// mock-ssh-server — headless fixture for the install-wizard tests.
#[derive(Parser, Debug)]
#[command(
    name = "mock-ssh-server",
    about = "Accept any payload, write to inbox. Not a real SSH server."
)]
struct Cli {
    /// Port to bind on. `0` = pick an ephemeral port (the test driver
    /// discovers the actual port via --ready-file or by listing
    /// /proc/net/tcp; see install.rs for the spawn helper).
    #[arg(long, default_value_t = 0)]
    port: u16,

    /// Inbox directory — one JSON file per install command.
    #[arg(long, default_value = "/tmp/mock-ssh-inbox")]
    inbox: PathBuf,

    /// Optional ready file. The server writes its actual bound port
    /// (one line of UTF-8) to this path once `bind()` succeeds, so the
    /// test driver can read it back. Skipped if `--no-ready-file` is
    /// passed (default: writes the file).
    #[arg(long)]
    ready_file: Option<PathBuf>,

    /// Exit after the first connection (used by tests; production
    /// smoke runs leave this off).
    #[arg(long, default_value_t = false)]
    one_shot: bool,
}

fn main() {
    let cli = Cli::parse();
    let inbox = Arc::new(cli.inbox);
    std::fs::create_dir_all(inbox.as_ref()).expect("create inbox dir");

    let bind_addr = format!("127.0.0.1:{}", cli.port);
    let listener = TcpListener::bind(&bind_addr).expect("bind 127.0.0.1:<port>");
    let local_port = listener.local_addr().expect("local_addr").port();
    eprintln!(
        "mock-ssh-server listening on 127.0.0.1:{local_port} (inbox={})",
        inbox.display()
    );

    if let Some(ready) = &cli.ready_file {
        // Write the actual bound port so the test driver can read it.
        if let Some(parent) = ready.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(ready, format!("{local_port}\n")).expect("write ready_file");
    }

    let seq = Arc::new(AtomicU64::new(0));
    let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let inbox = Arc::clone(&inbox);
                let seq = Arc::clone(&seq);
                handles.push(thread::spawn(move || {
                    if let Err(e) = handle_connection(s, inbox.as_ref(), &seq) {
                        eprintln!("mock-ssh-server connection error: {e}");
                    }
                }));
            }
            Err(e) => eprintln!("accept failed: {e}"),
        }
        if cli.one_shot {
            // Stop accepting new connections, but wait for the
            // handler thread to finish before the process exits —
            // otherwise the test driver's response read races the
            // process termination and gets ECONNRESET.
            drop(listener);
            for h in handles.drain(..) {
                let _ = h.join();
            }
            break;
        }
    }
}

fn handle_connection(mut stream: TcpStream, inbox: &Path, seq: &AtomicU64) -> std::io::Result<()> {
    // Read up to 64 KiB. The install payload is the rendered install
    // plan (collector.json + a `install` directive) — well under 64 KiB
    // for the unit-test use case. A real SSH install would be much
    // larger, but the wizard's "auto path" only sends a tiny stub.
    let mut buf = Vec::with_capacity(8 * 1024);
    let read_stream = (&mut stream).take(64 * 1024);
    let mut read_stream = read_stream; // binding to silence shadowing
    std::io::Read::read_to_end(&mut read_stream, &mut buf)?;

    // Split off the first line as collector_id; rest is payload.
    let (collector_id, payload) = match buf.iter().position(|&b| b == b'\n') {
        Some(n) => {
            let id = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            let rest = if n + 1 < buf.len() {
                buf[n + 1..].to_vec()
            } else {
                Vec::new()
            };
            (id, rest)
        }
        None => ("vps_collector".to_string(), buf),
    };

    let my_seq = seq.fetch_add(1, Ordering::SeqCst);
    let filename = inbox.join(format!("install-{my_seq:06}.json"));
    let record = json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "collector_id": collector_id,
        "payload": String::from_utf8_lossy(&payload).to_string(),
    });
    let mut f = std::fs::File::create(&filename)?;
    let body = serde_json::to_string_pretty(&record).unwrap_or_default();
    f.write_all(body.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;

    stream.write_all(b"ok\n")?;
    stream.flush()?;
    Ok(())
}
