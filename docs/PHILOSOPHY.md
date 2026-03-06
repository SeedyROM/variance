# The Philosophy of Variance

## Human Communication Is Ephemeral by Nature

When you have a conversation with someone — in person, on the phone, over coffee — it ends. The words disappear. You remember what mattered and forget the rest. That is not a limitation of human communication. That is how human communication is supposed to work.

Somewhere along the way, software decided that storing everything forever was a feature. It is not. It is a liability — for you, for the people you talk to, and for anyone who might one day need to prove what you said or didn't say. A chat application that retains years of your private conversations is not serving you. It is building a database at your expense.

Variance takes a different position. Conversations fade. By default, messages are gone after 30 days — not archived, not compressed, not moved to cold storage. Gone. This mirrors how humans actually communicate and removes the attack surface that comes with long-term message retention. You cannot leak what you do not have. A server cannot be compelled to produce records that do not exist.

This is not a technical limitation. It is a deliberate choice about what a communication tool should be.

---

## Deliberate Design Decisions

### Identity Without a Gatekeeper

Every major communication platform requires you to prove who you are to a company. A phone number. An email address. An account they can suspend, delete, or hand over to a third party on request. Your identity on those platforms is not yours — it is on loan.

Variance uses the W3C Decentralized Identifier standard (DID) as its identity layer. A DID is simply a unique identifier backed by a cryptographic keypair that you generate and control. Nobody issues it to you. Nobody can revoke it. It is not blockchain technology — it has nothing to do with cryptocurrency or financial systems. It is a practical, open standard for saying "this is me, provably, without asking anyone's permission." Think of it like an SSH key with a standardized format that other software can understand.

Your identity in Variance is a file on your device. Back it up. If you lose it, you lose your identity — there is no account recovery because there is no account. That is the tradeoff, and it is the right one.

### No Central Servers

There is no Variance Inc. server handling your messages. There is no API endpoint to block, no database to breach, no company to receive a court order. Messages travel peer-to-peer between the people in the conversation. When neither party is online simultaneously, an optional relay node holds the message temporarily — but the relay sees only encrypted bytes with a destination address. It cannot read the content.

Relay nodes can be run by anyone and automatically join the network. There is no central relay registry to take down.

### Contact Discovery That Does Not Require Trusting Us

Variance does not want to know who your friends are. There is no contact upload, no phone book sync, no social graph being harvested in the background. You find people the way humans have always found each other — you meet them somewhere and exchange identifiers. Share a username in person. Send a link through a channel you already trust. Scan a code. Variance is not the discovery layer and has no interest in becoming one.

### Ephemerality as Default

Message expiry in most applications is an opt-in setting buried in a menu that most users never find. In Variance it is the baseline. The 30-day window is not a punishment — it is a statement that your conversations belong to the moment they happened, not to a database that outlives the relationship.

---

## Censorship Resistance

The internet was designed to route around damage. A network that can survive a nuclear strike can survive an authoritarian government trying to silence a conversation — if the software is built with that in mind.

Blocking Variance requires blocking behaviors, not addresses. There is no variance.com to DDOS. There is no central API to firewall. There is no certificate to revoke. What an adversary faces instead:

- **DHT peer discovery** built on Kademlia — the same protocol underneath BitTorrent and IPFS, used by hundreds of millions of devices. Blocking it breaks a significant portion of the internet.
- **Relay nodes** that can be spun up on any server, in any jurisdiction, and automatically join the network. There is no relay registry to seize.
- **QUIC transport** alongside TCP — significantly harder to identify and throttle via deep packet inspection than cleartext protocols.
- **End-to-end encryption** throughout — intercepted traffic is useless without the keys, which never leave your device.

A nation state that wants to block this has to either shut down TCP/IP — breaking their own infrastructure in the process — or perform expensive per-connection analysis at scale with no guarantee of success. That is not a theoretical property. It is why well-resourced adversaries have struggled for years against systems built on the same principles.

---

## Technical Approach: Standing on Shoulders

Variance is not an exercise in reinventing cryptography. The hard problems in secure messaging have been worked on for decades by people far more specialized than any single developer. The correct response to that is to use their work.

**vodozemac** is the Olm/Double Ratchet implementation used by Matrix and Element in production. It provides the per-message forward secrecy for direct messages — if a session key is ever compromised, past messages remain protected.

**OpenMLS** implements RFC 9420, the IETF standard for Messaging Layer Security. It provides the group encryption layer — post-compromise security, forward secrecy, and cryptographic group membership without a trusted server.

**libp2p** is the networking stack used by IPFS and a significant portion of the decentralized web. It handles peer discovery, NAT traversal, transport selection, and connection management.

**Protocol Buffers** define all wire formats. Every message that crosses a network boundary has a typed, versioned schema.

None of these were invented for Variance. They are mature, widely deployed, and maintained by large communities. Using them means Variance benefits from every security audit, every bug fix, and every protocol improvement those communities produce. Writing your own Double Ratchet implementation is how you introduce subtle vulnerabilities that take years to find.

### Why Rust

Rust was chosen because it is fast, memory-safe without a garbage collector, and has an excellent ecosystem for the libraries above. A memory safety vulnerability in a privacy tool is not an acceptable tradeoff for development convenience. Beyond performance, the Rust ecosystem's approach to explicit error handling, ownership, and threading makes the codebase auditable — someone reading this code can reason about what it does and what it cannot do.

### Why AGPL-3.0

The GNU Affero General Public License version 3 was chosen deliberately. MIT and Apache licenses allow a company to take this code, run it as a service, and never release their modifications. AGPL closes that loophole: if you run a modified version of Variance as a network service, you must release your source code under the same license.

This is not a restriction on users. It is a restriction on exploitation. Anyone can read the code, run it, modify it, and build on it — they just cannot build a proprietary product on top of it and walk away. The goal is a commons, not a launching pad for someone else's data collection business.

---

## On Cryptography and Honest Security Claims

No cryptographic system is perfectly secure. Anyone who tells you otherwise is selling something.

What Variance can honestly claim: the protocols in use are the current state of the art in applied cryptography. Double Ratchet with Olm provides forward secrecy and break-in recovery for direct messages. MLS provides the same guarantees for groups. Ed25519 signatures authenticate identity. Argon2id protects your identity file at rest. These are not novel experiments — they are deployed at scale by organizations whose threat models are at least as serious as yours.

What Variance cannot protect against: a compromised device, a malicious contact, or someone looking over your shoulder. The protocol secures the wire. It cannot secure the endpoints. If your device is owned, your messages are readable. If you trust someone you should not trust, they can screenshot the conversation. Cryptography solves the problem of secure transmission between two parties. It does not solve the human problem of who those parties are and whether they can be trusted.

The honest position is that Variance makes mass surveillance expensive, targeted surveillance difficult, and opportunistic data collection impossible. It does not make you invisible. Nothing does.

Use it accordingly.
