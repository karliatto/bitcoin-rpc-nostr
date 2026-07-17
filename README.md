# ContextBTC Rust

ContextBTC Rust provides a [Model Context Protocol (MCP)](https://modelcontextprotocol.io) interface to a Bitcoin Core node, using [ContextVM](https://github.com/contextvm) to transport MCP messages over Nostr. Nostr's cryptographic keypairs and signed events provide built-in verification and authorization.

## Generating a Nostr key

The server needs a stable Nostr identity. Generate a secret key with [nak](https://github.com/fiatjaf/nak) (included in the dev shell):

```bash
nak key generate
# -> 7b94e287...bc6148d  (64-char hex secret key)
```

Derive the public key (what clients target) from a secret key with:

```bash
nak key public <secret-key-hex>
```

## Configuration

The server is configured via environment variables. For local development, copy
the provided template and fill in your values:

```bash
cp .env.example .env
# edit .env
```

On startup the server automatically loads a `.env` file if present. Real
environment variables always take precedence over `.env`, and a missing file is
not an error (useful for systemd/Docker where variables are injected directly).
`.env` is gitignored, so your secrets are never committed.

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `SERVER_NOSTR_SECRET_KEY` | No | ephemeral | 64-char hex or `nsec...` key. If unset, a temporary key is generated on each start (testing only, not for production). |
| `NOSTR_RELAY_URLS` | No | `ws://localhost:10547` | Comma-separated relay websocket URLs, used by both server and client. |
| `BITCOIN_RPC_URL` | No | `http://127.0.0.1:8332` | Bitcoin Core JSON-RPC endpoint. |
| `BITCOIN_RPC_USER` | Yes | — | JSON-RPC username. |
| `BITCOIN_RPC_PASSWORD` | Yes | — | JSON-RPC password. |
| `BITCOIN_RPC_TIMEOUT_SECS` | No | `30` | Overall HTTP request timeout for RPC calls, in seconds. |

## Project layout

This is a Cargo workspace with two binary crates:

- `crates/server` — the ContextBTC MCP server (`contextbtc-server`).
- `crates/client` — an example client (`contextbtc-client`).

## Running server

With a `.env` file in place:

```bash
cargo run -p contextbtc-server
```

Alternatively, set variables inline (these override any `.env` values):

```bash
SERVER_NOSTR_SECRET_KEY=<secret-key-hex> \
BITCOIN_RPC_URL=http://127.0.0.1:18443 \
BITCOIN_RPC_USER=myuser \
BITCOIN_RPC_PASSWORD=mypass \
cargo run -p contextbtc-server
```

## Running client

## Client .env

```bash
CLIENT_NOSTR_SECRET_KEY=
```

```bash
cargo run -p contextbtc-client -- <server-pub-key-hex>
```

## Running with Nix (from another machine)

The flake exposes prebuilt packages, so any machine with [Nix](https://nixos.org/download)
(flakes enabled) can run the server or client straight from GitHub — no clone,
no toolchain setup:

```bash
# Run the server
nix run github:karliatto/contextbtc

# Run the client (note the `--` before program arguments)
nix run github:karliatto/contextbtc#client -- <server-pub-key-hex>
```

Configuration works the same way as a local run: pass the environment variables
from the [Configuration](#configuration) table inline, e.g.

```bash
SERVER_NOSTR_SECRET_KEY=<secret-key-hex> \
NOSTR_RELAY_URLS=wss://relay.contextvm.org \
BITCOIN_RPC_URL=http://127.0.0.1:8332 \
BITCOIN_RPC_USER=myuser \
BITCOIN_RPC_PASSWORD=mypass \
nix run github:karliatto/contextbtc
```

To build without running, or to install into your profile:

```bash
nix build github:karliatto/contextbtc   # -> ./result/bin/{contextbtc-server,contextbtc-client}
nix profile install github:karliatto/contextbtc
```

### As a NixOS service

For a NixOS host, the flake also provides a module (`nixosModules.default`) that
runs the server as a hardened systemd service. Add it to the target machine's
flake:

```nix
{
  inputs.contextbtc.url = "github:karliatto/contextbtc";

  outputs = { nixpkgs, contextbtc, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        contextbtc.nixosModules.default
        {
          services.contextbtc = {
            enable = true;
            relayUrls = [ "wss://relay.contextvm.org" ];
            # Non-secret settings:
            extraEnvironment.BITCOIN_RPC_URL = "http://127.0.0.1:8332";
            # Secrets (SERVER_NOSTR_SECRET_KEY, BITCOIN_RPC_USER/PASSWORD, ...)
            # live in a file read at runtime, never in the Nix store:
            environmentFile = "/run/secrets/contextbtc.env";
          };
        }
      ];
    };
  };
}
```

Then `sudo nixos-rebuild switch`. The service runs as an isolated `DynamicUser`
with automatic restart.

## Architecture

This project bridges two distinct protocol layers:

- **Client ⟷ ContexVM MCP server:** MCP over Nostr.
- **ContexVM MCP server ⟷ bitcoind:** JSON-RPC over HTTP.
