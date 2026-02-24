# Plan: NAT Traversal — Circuit Relay + DCUTR + `variance-relay` Crate

## Context

Two Variance nodes on different networks (or behind NAT) cannot connect directly: mDNS is LAN-only, and most home/office routers block inbound connections. The fix is libp2p circuit relay v2 + DCUTR (direct connection upgrade through relay):

- A relay node (public IP) lets NATed peers rendezvous and relay traffic
- DCUTR then attempts hole-punching to upgrade the relayed connection to a direct one
- If hole-punching fails, traffic continues through the relay

A new `variance-relay` binary crate provides the relay node to run on a VPS.

---

## Files to Create

- `crates/variance-relay/Cargo.toml`
- `crates/variance-relay/src/main.rs`

## Files to Modify

1. `Cargo.toml` (workspace root) — add `relay`/`dcutr` features to libp2p; add `variance-relay` to workspace members
2. `crates/variance-p2p/Cargo.toml` — add `relay`/`dcutr` to libp2p features
3. `crates/variance-p2p/src/config.rs` — add `relay_peers: Vec<BootstrapPeer>` field
4. `crates/variance-p2p/src/behaviour.rs` — add `relay_client` and `dcutr` fields
5. `crates/variance-p2p/src/node/mod.rs` — SwarmBuilder chain, relay dialing, circuit listen, Node struct
6. `crates/variance-p2p/src/node/event_handlers.rs` — relay client + dcutr event handlers

---

## Step 1 — Cargo.toml (workspace root)

Add `variance-relay` to `[workspace].members`.

Add `relay` and `dcutr` to the workspace libp2p features:
```toml
libp2p = { version = "0.55", features = [
    ..., "relay", "dcutr"
] }
```

---

## Step 2 — `crates/variance-p2p/Cargo.toml`

Add `relay` and `dcutr` to the libp2p dependency features (mirrors workspace).

---

## Step 3 — `config.rs`

Add one field to `Config`:
```rust
/// Relay nodes for NAT traversal. After connecting, the node will
/// reserve a circuit slot and be reachable at the relay's circuit address.
#[serde(default)]
pub relay_peers: Vec<BootstrapPeer>,
```
Reuses the existing `BootstrapPeer` type (same shape: `peer_id: String`, `multiaddr: Multiaddr`).

Default: empty vec. No other defaults change.

---

## Step 4 — `behaviour.rs`

Add two fields to `VarianceBehaviour`:
```rust
pub relay_client: relay::client::Behaviour,
pub dcutr: dcutr::Behaviour,
```

Both derive automatically via `#[derive(NetworkBehaviour)]`.

---

## Step 5 — `node/mod.rs`

### 5a. Imports
Add to libp2p use: `dcutr`, `relay`.

### 5b. Node struct — add relay peer tracking
```rust
/// Peer IDs of configured relay nodes, used to trigger circuit listen after connection
relay_peer_ids: std::collections::HashSet<PeerId>,
```
Initialize from `config.relay_peers` during `Node::new()`.

### 5c. SwarmBuilder chain — restructure for relay client

`.with_relay_client()` must be inserted after transports and changes the `.with_behaviour()` closure signature from `|_keypair|` to `|keypair, relay_client|`. Move all behaviour construction inside the closure (they already have access to `peer_id` via `keypair.public().to_peer_id()`):

```rust
let swarm = SwarmBuilder::with_existing_identity(keypair)
    .with_tokio()
    .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)
    .map_err(...)?
    .with_quic()
    .with_relay_client(noise::Config::new, yamux::Config::default)
    .map_err(...)?
    .with_behaviour(|keypair, relay_client| {
        let peer_id = keypair.public().to_peer_id();
        // build kad, gossipsub, mdns, identify, ping, custom protocols
        // ... (same logic as today, just moved inside closure)
        Ok(VarianceBehaviour {
            relay_client,
            dcutr: dcutr::Behaviour::new(peer_id),
            kad, gossipsub, mdns, identify, ping,
            identity, offline_messages, signaling, direct_messages, typing_indicators,
        })
    })
    .map_err(...)?
    .with_swarm_config(|c| { ... })
    .build();
```

Error handling: the closure returns `Result<VarianceBehaviour, Error>` — libp2p propagates it via the `?` on `.with_behaviour(...)`.

### 5d. `listen()` — dial relay peers

After the existing DHT bootstrap block:
```rust
for relay in &config.relay_peers {
    let peer_id: PeerId = relay.peer_id.parse()...;
    self.swarm.behaviour_mut().kad.add_address(&peer_id, relay.multiaddr.clone());
    self.swarm.dial(relay.multiaddr.clone()).map_err(...)?;
    info!("Dialing relay peer: {} at {}", peer_id, relay.multiaddr);
}
```

Circuit `listen_on` happens in the connection event (see §6) once the relay is actually connected.

---

## Step 6 — `event_handlers.rs`

### 6a. `handle_behaviour_event` dispatch

Add two arms:
```rust
VarianceBehaviourEvent::RelayClient(e) => self.handle_relay_client_event(e),
VarianceBehaviourEvent::Dcutr(e) => self.handle_dcutr_event(e),
```

### 6b. `handle_relay_client_event`

```rust
fn handle_relay_client_event(&mut self, event: relay::client::Event) {
    match event {
        relay::client::Event::ReservationReqAccepted { relay_peer_id, .. } => {
            info!("Relay reservation accepted: reachable via {}", relay_peer_id);
        }
        relay::client::Event::ReservationReqFailed { relay_peer_id, error, .. } => {
            warn!("Relay reservation failed for {}: {:?}", relay_peer_id, error);
        }
        relay::client::Event::OutboundCircuitEstablished { relay_peer_id, .. } => {
            debug!("Outbound circuit established via relay {}", relay_peer_id);
        }
        relay::client::Event::OutboundCircuitReqFailed { relay_peer_id, error, .. } => {
            debug!("Outbound circuit failed via relay {}: {:?}", relay_peer_id, error);
        }
        _ => {}
    }
}
```

### 6c. `handle_dcutr_event`

```rust
fn handle_dcutr_event(&mut self, event: dcutr::Event) {
    match event {
        dcutr::Event::DirectConnectionUpgradeSucceeded { remote_peer_id } => {
            info!("Direct connection established with {} (hole punch succeeded)", remote_peer_id);
        }
        dcutr::Event::DirectConnectionUpgradeFailed { remote_peer_id, error } => {
            debug!("Hole punch failed with {}, staying on relay: {:?}", remote_peer_id, error);
        }
        _ => {}
    }
}
```

### 6d. `handle_connection_established` — trigger circuit listen

After the existing auto-discovery identity request, add:
```rust
if self.relay_peer_ids.contains(&peer_id) {
    // Reserve a circuit slot on this relay so we're reachable via it
    let circuit_addr: Multiaddr = format!("/p2p/{}/p2p-circuit", peer_id).parse()
        .expect("valid circuit addr");
    if let Err(e) = self.swarm.listen_on(circuit_addr) {
        warn!("Failed to listen on relay circuit for {}: {}", peer_id, e);
    }
}
```

---

## Step 7 — `variance-relay` crate

A standalone binary. Does **not** depend on `variance-p2p` or any Variance business logic — just libp2p directly.

### `Cargo.toml`
```toml
[package]
name = "variance-relay"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "variance-relay"
path = "src/main.rs"

[dependencies]
libp2p = { workspace = true, features = ["relay", "identify", "ping", "tcp", "quic", "noise", "yamux", "tokio", "macros"] }
tokio = { workspace = true, features = ["full"] }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }
clap = { version = "4.5", features = ["derive"] }
serde_json = { workspace = true }
```

Identity persisted as a JSON file (libp2p keypair bytes) in the data directory so the peer ID is stable across restarts.

### `src/main.rs` — structure

```
#[derive(NetworkBehaviour)]
struct RelayBehaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
}
```

CLI args (clap):
- `--port` (default 4001) — TCP listen port (QUIC uses same port number)
- `--data-dir` (default `~/.variance-relay`) — where to persist keypair

Startup sequence:
1. Load or generate Ed25519 keypair from `<data-dir>/keypair.json`
2. Print peer ID and listen multiaddrs
3. Build swarm with `relay::Behaviour::new(peer_id, Default::default())`
4. `listen_on("/ip4/0.0.0.0/tcp/<port>")` + `listen_on("/ip4/0.0.0.0/udp/<port>/quic-v1")`
5. Print the full multiaddrs clients should configure (resolved addresses after `NewListenAddr`)
6. Run event loop; log relay reservations and circuit events at info level
7. Graceful shutdown on Ctrl+C

**On startup, print:**
```
Relay peer ID: 12D3KooW...
Listening on:
  /ip4/0.0.0.0/tcp/4001/p2p/12D3KooW...
  /ip4/0.0.0.0/udp/4001/quic-v1/p2p/12D3KooW...

Add to client config:
  relay_peers = [{ peer_id = "12D3KooW...", multiaddr = "/ip4/<YOUR_IP>/tcp/4001" }]
```

---

## Verification

1. `cargo check -p variance-p2p` — no compile errors
2. `cargo check -p variance-relay` — no compile errors
3. Run relay on a machine with a known IP: `variance-relay --port 4001`
4. Configure both desktop clients with that relay peer in their config
5. Launch both clients — verify logs show `Relay reservation accepted`
6. Verify `DirectConnectionUpgradeSucceeded` or fallback relay traffic in logs
7. Send a message between the two clients across networks

For local testing without a VPS: run `variance-relay` on the same machine, use `127.0.0.1` as the relay address — both local clients will relay through it and attempt DCUTR.
