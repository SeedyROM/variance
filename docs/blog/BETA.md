# Decentralized Messaging for the Post-Platform Internet

I built a chat application. There are a lot of those. Here's why I think this one matters.

Variance is a peer-to-peer, end-to-end encrypted messaging app. No accounts, no phone numbers, no central server holding your conversations. You generate a cryptographic identity on your device, pick a username, and start talking directly to people. Messages are encrypted before they leave your machine and decrypted on the recipient's machine. Nothing in between can read them, store them, or hand them over to anyone.

It's a desktop app right now. Tauri frontend, Rust backend, everything running in-process. No Electron bloat, no sidecar process, no phoning home. You install it, it generates a libp2p node, and you're on the network. There is no token. There is no blockchain. There is nothing to invest in.

## The Problem Isn't Any Specific Company

I'm not going to spend 800 words telling you why Discord or Telegram or Slack is bad. You already know or you don't care, and either way the argument is the same.

The structural problem with centralized messaging is that it requires you to trust a third party with your social graph, your message history, and your identity. That trust is load-bearing. If the company gets breached, your data is exposed. If they get acquired, your data changes hands. If they receive a legal order, they comply because they have no choice and because the data exists to be handed over. If they decide your account violates their terms, your identity on that platform ceases to exist.

This isn't about bad actors. It's about architecture. A system that stores your conversations on someone else's computer has a fundamentally different threat model than one that doesn't. Variance doesn't.

## Identity Without Permission

Every centralized platform requires you to prove who you are to a company. A phone number, an email, an account they can suspend or delete at will. Your identity on those platforms isn't yours. It's on loan.

The web3 crowd talked a lot about self-sovereign identity. They were right about the problem. The idea that you should own your identity and not rent it from a platform is a good one. But the solution was never a $200 million token launch backed by Andreessen Horowitz. It was never an NFT profile picture or a DAO governance vote to decide your display name. The useful kernel of that entire movement was "your keys, your identity," and it got buried under layers of financialization that existed to make VCs and early adopters rich. The sovereignty was the point. The tokenomics were the grift.

And notice how fast the "decentralization" crowd pivoted to crypto trading when the money showed up. The same people who said the internet needed to be trustless and private built their identities on a public ledger that records every transaction forever. "Privacy" on a blockchain meant Tornado Cash, and even that was traceable. HKDF-derived wallets, burner addresses, mixing services, none of it actually worked. I've seen the other side of this firsthand. The company I worked at had metrics and queries specifically designed to trace these "anonymous" paper trails, because the trails exist and they exist permanently. The blockchain doesn't forget. That's the entire point of a blockchain. So the privacy story was always a fiction, and the people selling it knew that. They weren't building privacy tools. They were building casinos with extra steps, and you were supposed to gamble your way to sovereignty.

Variance uses the W3C Decentralized Identifier standard (DID). A DID is a unique identifier backed by an Ed25519 keypair that you generate locally. Nobody issues it. Nobody can revoke it. There's no token attached, no wallet to connect, no gas fees, no speculative asset masquerading as infrastructure. Think of it like an SSH key with a standardized format that other software can understand. That's it. The cryptography that actually matters for identity has existed for decades. It just needed to be used without a fundraising round attached.

Your identity is a file on your device, protected at rest with Argon2id. Back it up, or derive it from a BIP39 mnemonic phrase. If you lose both, you lose your identity. There is no "forgot password" flow, because there is no account. That's the tradeoff, and it's the right one.

## Conversations Should Fade

When you talk to someone over coffee, the conversation ends. You remember what mattered and forget the rest. That's not a limitation of human communication. That's how it works.

Somewhere along the way, software decided that storing everything forever was a feature. It's not. It's a liability. A chat application that retains years of your private conversations is building a database at your expense. You cannot leak what you do not have. A server cannot be compelled to produce records that do not exist.

Messages in Variance expire after 30 days by default. Not archived, not moved to cold storage. Gone. This isn't a technical limitation. It's a design decision about what a communication tool should be.

## The Encryption Is Not Homegrown

I didn't write any cryptography. The hard problems in secure messaging have been worked on by specialists for decades, and the correct response is to use their work.

**Direct messages** use vodozemac, the Olm/Double Ratchet implementation used in production by Matrix and Element. Every message gets a new key. If a session key is somehow compromised, past messages stay protected. That's forward secrecy in practice, not in a whitepaper.

**Group messages** use OpenMLS, an implementation of RFC 9420, the IETF's Messaging Layer Security standard. It gives groups the same forward secrecy guarantees as direct messages, plus post-compromise security. If a member's device is compromised and later secured, future messages become unreadable to the attacker after the next key update. I originally built group encryption with hand-rolled AES-256-GCM. I ripped it out and replaced it with MLS, because rolling your own group crypto is how you introduce subtle vulnerabilities that take years to find.

**Networking** runs on libp2p, the same stack underneath IPFS and a significant chunk of the decentralized web. Kademlia DHT for peer discovery, GossipSub for group message fan-out, QUIC and TCP transports, NAT traversal via relay nodes and DCUtR (direct connection upgrade through relay).

**Wire format** is Protocol Buffers everywhere. Every message that crosses a network boundary has a typed, versioned schema. No ad-hoc JSON parsing, no version mismatch surprises.

## The Relays Know Nothing

When two people aren't online at the same time, messages need somewhere to wait. Variance uses optional relay nodes for this. Here's what a relay sees:

- A blob of encrypted bytes.
- A mailbox token: a SHA-256 hash of the recipient's public signing key.

That's it. The relay cannot read the message content. It doesn't know who sent it. It doesn't know the recipient's DID or username. It sees a hash and some ciphertext, holds them for up to 30 days, and delivers them when someone shows up with the corresponding key. After 30 days, the data is deleted.

Relay nodes can be run by anyone. There's no central registry, no permission required, no special infrastructure. You build the binary, point it at a port, and it joins the network. If one relay goes down, traffic routes to others. If a government seizes one, they get encrypted bytes with no metadata pointing anywhere useful.

If this sounds familiar, it should. The relay model borrows from the same lineage as Tor's onion routing and I2P's garlic routing: nodes that forward traffic without understanding it. The difference right now is that Variance relays are single-hop. The roadmap includes multi-hop relay chains, where a message bounces through multiple independent nodes before reaching its destination, so that no single relay can correlate sender and receiver even by timing analysis. libp2p's transport-agnostic design makes this feasible without rearchitecting the protocol layer. The plumbing is there. It's a matter of building the routing logic on top of it.

## Censorship Resistance as an Engineering Property

This isn't a theoretical claim. It's a consequence of the architecture.

Blocking Variance requires blocking behaviors, not addresses. There's no variance.com to take down, no central API to firewall, no certificate to revoke. Peer discovery runs on Kademlia, the same protocol underneath BitTorrent. Blocking it breaks a meaningful portion of the internet. QUIC transport is significantly harder to fingerprint via deep packet inspection than cleartext protocols. Relay nodes can be spun up in any jurisdiction. End-to-end encryption means intercepted traffic is useless without keys that never leave the device.

A well-resourced adversary who wants to stop this has to either shut down TCP/IP or perform expensive per-connection analysis at scale with no guarantee of success. That's not because I'm clever. It's because these protocols were designed by people who were thinking about exactly this problem.

## Honest Limitations

No cryptographic system is perfectly secure. If someone tells you otherwise, they're selling something.

Variance makes mass surveillance expensive, targeted surveillance difficult, and opportunistic data collection impossible. It does not make you invisible.

If your device is compromised, your messages are readable. If you trust someone you shouldn't, they can screenshot the conversation. If you lose your identity file and your mnemonic, your identity is gone. There's no cloud backup for messages. Group messages sent while you're offline aren't automatically synced yet. WebRTC audio/video has the signaling protocol built but the actual media streams aren't wired up yet.

These are real limitations, not caveats buried in fine print. Use it accordingly.

## Why Rust, Why AGPL

Rust because a memory safety vulnerability in a privacy tool is not an acceptable tradeoff for development convenience. The ownership model and explicit error handling also make the codebase auditable. Someone reading this code can reason about what it does and what it cannot do.

AGPL-3.0 because MIT and Apache let a company take this code, run it as a service, and never release their modifications. AGPL closes that loophole. If you run a modified version as a network service, you release your source. This isn't a restriction on users. It's a restriction on exploitation. The goal is a commons, not a launching pad for someone else's data collection business.

## Where It Stands

Variance has been in active development for about a month. ~36,000 lines of Rust, ~8,500 of TypeScript, 700+ tests (unit, integration, e2e), and it works. Direct messaging, group messaging, relay infrastructure, NAT traversal, desktop app with onboarding, message reactions, read receipts, typing indicators, presence tracking, QR contact sharing. It's not a prototype. It's early software.

The code is on GitHub. Run it, break it, tell me what's wrong. If the ideas here matter to you, the best thing you can do is use it and file issues.

[GitHub: variance](https://github.com/SeedyROM/variance) | [Philosophy](https://github.com/SeedyROM/variance/blob/main/docs/PHILOSOPHY.md) | License: AGPL-3.0
