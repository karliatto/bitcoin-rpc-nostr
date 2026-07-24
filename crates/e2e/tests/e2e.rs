//! End-to-end test exercising the full stack:
//!
//! ```text
//!   contextbtc-client ──MCP/Nostr──▶ nak serve (relay) ──▶ contextbtc-server ──JSON-RPC──▶ bitcoind (regtest)
//! ```
//!
//! It starts a local Nostr relay (`nak serve`), a regtest `bitcoind` (via
//! `corepc-node`), runs the real server binary against that node, then runs the
//! real client binary and asserts it received live data from the node.
//!
//! The test requires a `bitcoind` binary (located via `BITCOIND_EXE` or `PATH`)
//! and `nak` on `PATH`; it fails if either is missing. The Nix devShell provides
//! both, so run it with `nix develop --command cargo test`.

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

/// Fixed server identity so the run is deterministic and warning-free (no
/// ephemeral key). This is a throwaway test key, not a secret.
const SERVER_SECRET_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";

/// Kills a spawned child when it goes out of scope so a failed assertion never
/// leaves `nak` or the server running.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Grab a free TCP port by binding to :0 and immediately releasing it. There's
/// an inherent race before the port is reused, but it's fine for a local test.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Poll until something is listening on `port`, or the deadline passes.
fn wait_for_tcp(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Whether `nak` is runnable.
fn nak_available() -> bool {
    Command::new("nak")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn server_client_roundtrip_over_nostr() -> anyhow::Result<()> {
    // --- Preconditions: both external tools must be present, or fail loudly. --
    let bitcoind_exe = corepc_node::exe_path()
        .expect("bitcoind not found: set BITCOIND_EXE or add bitcoind to PATH");
    assert!(
        nak_available(),
        "`nak` not found on PATH (needed to run the local relay via `nak serve`)"
    );

    // --- 1. Local Nostr relay (nak serve) -------------------------------------
    let relay_port = free_port();
    let relay_url = format!("ws://localhost:{relay_port}");
    let nak = Command::new("nak")
        .args(["serve", "--quiet", "--port", &relay_port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let _nak_guard = ChildGuard(nak);
    assert!(
        wait_for_tcp(relay_port, Duration::from_secs(10)),
        "relay did not start listening on {relay_port}"
    );

    // --- 2. Regtest bitcoind --------------------------------------------------
    // No wallet: the proxied tools are read-only chain/mempool queries, and
    // creating the default wallet fails on recent Bitcoin Core versions.
    let mut conf = corepc_node::Conf::default();
    conf.wallet = None;
    let node = corepc_node::Node::with_conf(&bitcoind_exe, &conf)?;
    let rpc_url = node.rpc_url();
    let cookie = node
        .params
        .get_cookie_values()?
        .expect("regtest node should expose cookie credentials");

    // --- 3. Build the workspace binaries --------------------------------------
    let server_bin = escargot::CargoBuild::new()
        .package("contextbtc-server")
        .bin("contextbtc-server")
        .run()?;
    let client_bin = escargot::CargoBuild::new()
        .package("contextbtc-client")
        .bin("contextbtc-client")
        .run()?;

    // --- 4. Start the server against bitcoind + the relay ---------------------
    let mut server = server_bin
        .command()
        .env("SERVER_NOSTR_SECRET_KEY", SERVER_SECRET_KEY)
        .env("NOSTR_RELAY_URLS", &relay_url)
        .env("BITCOIN_RPC_URL", &rpc_url)
        .env("BITCOIN_RPC_USER", &cookie.user)
        .env("BITCOIN_RPC_PASSWORD", &cookie.password)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    // Stream the server's stdout on a thread so we can watch for the pubkey it
    // prints at startup.
    let stdout = server.stdout.take().expect("server stdout piped");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let _server_guard = ChildGuard(server);

    // We only wait for the pubkey line here. The server's own "Server ready"
    // log comes from `serve()`, which completes the MCP initialize handshake —
    // and that only happens once a client connects. Blocking on it here would
    // deadlock against the client we start below.
    let mut server_pubkey: Option<String> = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && server_pubkey.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                if let Some(pk) = line.strip_prefix("Public key: ") {
                    server_pubkey = Some(pk.trim().to_string());
                }
            }
            Err(_) => break,
        }
    }
    let server_pubkey = server_pubkey.expect("server should print its public key");

    println!("Server pub key: {}", server_pubkey);

    // Give the server a moment to finish subscribing on the relay before the
    // client fires its first request.
    std::thread::sleep(Duration::from_secs(1));

    // --- 5. Run the client and assert it got live regtest data ----------------
    let mut client = client_bin
        .command()
        .arg(&server_pubkey)
        .env("NOSTR_RELAY_URLS", &relay_url)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let client_stdout = client.stdout.take().expect("client stdout piped");
    let (out_tx, out_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = String::new();
        for line in BufReader::new(client_stdout).lines().map_while(Result::ok) {
            buf.push_str(&line);
            buf.push('\n');
        }
        let _ = out_tx.send(buf);
    });

    let status = match client.wait_timeout(Duration::from_secs(30))? {
        Some(status) => status,
        None => {
            client.kill()?;
            client.wait()?;
            panic!("client did not finish within 30s (likely could not reach the server)");
        }
    };
    let output = out_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();

    println!("{}", status.success());
    println!("output {}", output);

    assert!(
        status.success(),
        "client exited with failure: {status:?}\n--- client stdout ---\n{output}"
    );
    assert!(
        output.contains("\"chain\":\"regtest\""),
        "client output missing regtest blockchain info:\n{output}"
    );
    assert!(
        output.contains("Block count:"),
        "client output missing block count:\n{output}"
    );

    Ok(())
}
