# Variance CLI Usage Guide

#### *This is only used for debugging/testing purposes. The primary user interface is the Tauri desktop app which embeds the [`variance-app`](../crates/variance-app) internally.*

## Installation

```bash
cargo build --release
# Binary will be at: target/release/variance
```

## Quick Start

1. **Generate an identity:**
   ```bash
   variance identity generate
   ```
   This creates `.variance/identity.json` with your DID and signing keys.

   **⚠️ IMPORTANT:** Write down the 12-word recovery phrase shown! This is the only time it will be displayed.

2. **Initialize configuration (optional):**
   ```bash
   variance config init
   ```
   This creates `config.toml` with default settings.

3. **Start the node:**
   ```bash
   variance start
   ```
   The node automatically loads your identity from `.variance/identity.json`.

## Commands

### Start Node

Start the Variance node with HTTP API:

```bash
variance start [OPTIONS]
```

**Options:**
- `-c, --config <FILE>` - Path to config file (default: `config.toml`)
- `-l, --listen <ADDR>` - HTTP API address (default: from config or `127.0.0.1:3000`)
- `-d, --did <DID>` - Override DID (optional, for testing only)

The node automatically loads your identity from the path specified in `config.toml` (default: `.variance/identity.json`).

**Examples:**
```bash
# Start with default settings
variance start

# Start with custom listen address
variance start --listen 127.0.0.1:8080

# Start with custom config file
variance start --config my-config.toml
```

**Shutdown:**
Press `Ctrl+C` for graceful shutdown.

### Configuration Management

#### Initialize Config

Create a new configuration file:

```bash
variance config init [OPTIONS]
```

**Options:**
- `-o, --output <FILE>` - Output path (default: `config.toml`)
- `-f, --force` - Overwrite existing file

**Example:**
```bash
variance config init --output my-config.toml
```

#### Show Config

Display current configuration:

```bash
variance config show [OPTIONS]
```

**Options:**
- `-c, --config <FILE>` - Config file to display (default: `config.toml`)

### Identity Management

#### Generate Identity

Create a new DID and signing key with BIP39 recovery phrase:

```bash
variance identity generate [OPTIONS]
```

**Options:**
- `-o, --output <FILE>` - Output file (default: `.variance/identity.json`)
- `-f, --force` - Overwrite existing file

**Examples:**
```bash
# Generate identity at default location (.variance/identity.json)
variance identity generate

# Generate at custom location
variance identity generate --output alice.json
```

**What happens:**
1. A 12-word BIP39 recovery phrase is generated
2. Your signing keys are derived from this phrase
3. The recovery phrase is displayed **once** - write it down!
4. The identity file is saved to disk

**⚠️ CRITICAL Security Notes:**
- **Write down the 12-word recovery phrase** shown during generation
- This phrase will **NEVER be shown again**
- Store it on paper in a safe place (NOT digitally)
- Anyone with these 12 words can recover your identity
- The identity file contains your private signing key - keep it secure!

#### Recover Identity

Recover your identity from a 12-word BIP39 recovery phrase:

```bash
variance identity recover [OPTIONS]
```

**Options:**
- `-o, --output <FILE>` - Output file (default: `.variance/identity.json`)
- `-f, --force` - Overwrite existing file

**Example:**
```bash
# Recover identity to default location
variance identity recover

# You'll be prompted to enter your 12 words:
# > witch collapse practice feed shame open despair creek road again ice least
```

**When to use:**
- Lost your `.variance/identity.json` file
- Setting up on a new device
- Restoring from backup

**Important:**
- Enter your 12 words in the correct order
- The recovered DID will match your original DID
- The signing keys will be identical to the original

#### Show Identity

Display identity information:

```bash
variance identity show [OPTIONS]
```

**Options:**
- `-i, --input <FILE>` - Identity file (default: `.variance/identity.json`)

## Configuration File

The `config.toml` file contains settings for:

- **Server**: HTTP API host and port
- **P2P**: libp2p listen addresses and bootstrap peers
- **Identity**: IPFS API endpoint and cache settings
- **Media**: STUN/TURN servers for WebRTC
- **Storage**: Paths for data storage

Example `config.toml`:

```toml
[server]
host = "127.0.0.1"
port = 3000

[p2p]
listen_addrs = ["/ip4/0.0.0.0/tcp/0"]
bootstrap_peers = []

[identity]
ipfs_api = "http://127.0.0.1:5001"
cache_ttl_secs = 3600

[media]
stun_servers = [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
]
turn_servers = []

[storage]
base_dir = ".variance"
identity_path = ".variance/identity.json"
identity_cache_dir = ".variance/identity_cache"
message_db_path = ".variance/messages.db"
```

## Desktop App (Tauri)

The desktop application (`app/`) does **not** use the `variance` CLI binary. Instead it embeds the node
**in-process** by linking directly against `variance_app` as a Rust library.

### How it works

On launch the Tauri host:

1. Checks for an existing identity file at the platform data directory:
   - **macOS**: `~/Library/Application Support/variance/identity.json`
   - **Linux**: `~/.local/share/variance/identity.json`
2. If no identity exists, it shows the **onboarding flow** (generate or recover via BIP39 mnemonic)
3. Once identity is confirmed, it calls `start_node()` from `variance_app` directly — no subprocess, no port configuration needed
4. The HTTP server binds to `127.0.0.1:0` (OS-assigned random port) and the UI connects via WebSocket on that port

### Tauri Commands (exposed to the UI)

| Command | Description |
|---------|-------------|
| `has_identity(path)` | Check if identity file exists |
| `generate_identity(path)` | Create new DID + 12-word mnemonic, write to disk |
| `recover_identity(mnemonic, path)` | Restore identity from BIP39 phrase |
| `default_identity_path()` | Returns platform-appropriate identity path |
| `start_node(identity_path)` | Start P2P node + HTTP API; returns assigned port |
| `stop_node()` | Graceful shutdown |
| `get_api_port()` | Returns current HTTP port (if running) |
| `get_node_status()` | Returns `{ running, local_did, api_port }` |

### Key differences from CLI usage

| Aspect | CLI | Tauri Desktop |
|--------|-----|---------------|
| Node startup | `variance start` binary | `start_node()` Tauri command (in-process) |
| Identity generation | `variance identity generate` | Onboarding flow → `generate_identity` command |
| Identity recovery | `variance identity recover` | Onboarding flow → `recover_identity` command |
| Identity path | Configurable (`--output`) | Fixed platform data dir |
| HTTP port | Configurable (`--listen`) | OS-assigned random port (`127.0.0.1:0`) |
| Config file | `config.toml` | Hardcoded defaults (no config file needed) |

The Tauri app and the CLI are **fully interoperable** — the identity file format is identical, so an
identity generated with `variance identity generate` can be copied to the platform data directory and
used by the desktop app, and vice versa.

## HTTP API

Once the node is running, the HTTP API is available at the configured address (default: `http://127.0.0.1:3000`).

### Available Endpoints

**Health Check:**
- `GET /health` - Service status

**Calls:**
- `POST /calls/create` - Initiate a call
- `GET /calls/active` - List active calls
- `POST /calls/{id}/accept` - Accept incoming call
- `POST /calls/{id}/reject` - Reject incoming call
- `POST /calls/{id}/end` - End active call

**Signaling:**
- `POST /signaling/offer` - Send WebRTC offer
- `POST /signaling/answer` - Send WebRTC answer
- `POST /signaling/ice` - Send ICE candidate
- `POST /signaling/control` - Send call control message

**Receipts:**
- `POST /receipts/delivered` - Send delivery receipt
- `POST /receipts/read` - Send read receipt
- `GET /receipts/{message_id}` - Get receipts for message

**Typing Indicators:**
- `POST /typing/start` - Start typing
- `POST /typing/stop` - Stop typing
- `GET /typing/{recipient}` - Get typing users

## Environment Variables

Control logging with `RUST_LOG`:

```bash
# Info level (default)
RUST_LOG=variance=info variance start

# Debug level
RUST_LOG=variance=debug variance start

# Trace level
RUST_LOG=variance=trace variance start
```

## Examples

### Development Workflow

```bash
# 1. Generate identity (saves to .variance/identity.json)
variance identity generate

# 2. View identity
variance identity show

# 3. Create config (optional)
variance config init

# 4. Start node (automatically loads identity)
variance start

# 5. Test API (in another terminal)
curl http://localhost:3000/health
```

### Production Deployment

```bash
# 1. Create production identity securely
variance identity generate --output /etc/variance/identity.json

# 2. Set appropriate permissions
chmod 600 /etc/variance/identity.json

# 3. Create production config
variance config init --output /etc/variance/config.toml

# 4. Edit config to set identity_path and production settings
vim /etc/variance/config.toml
# Set: storage.identity_path = "/etc/variance/identity.json"

# 5. Start with production settings
variance start \
  --config /etc/variance/config.toml \
  --listen 0.0.0.0:3000
```

## Troubleshooting

### Port Already in Use

If you get "address already in use", either:
1. Stop the existing process using the port
2. Use a different port: `variance start --did <DID> --listen 127.0.0.1:3001`

### Database Lock Error

If you get "could not acquire lock", ensure no other Variance instance is running with the same database path.

### IPFS Not Running

IPFS/IPNS integration is not yet implemented. Identity documents are stored in-memory only.
This section will be updated once IPFS integration lands.

### Lost Identity File

If you lost your `.variance/identity.json` file but have your 12-word recovery phrase:

```bash
variance identity recover
```

Enter your 12 words when prompted. Your identity will be fully restored with the same DID.

### Invalid Recovery Phrase

If you get "Invalid mnemonic phrase" when recovering:
1. Check that you have exactly 12 words
2. Verify the words are spelled correctly
3. Ensure they're in the correct order
4. Make sure there are no extra spaces or punctuation
