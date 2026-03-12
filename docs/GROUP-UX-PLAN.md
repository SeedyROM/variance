# Group UX Plan: Roles, Permissions & Invitation Flow

> Status: **Planned** | Created: 2026-03-11

## Overview

The group chat system has working MLS encryption, GossipSub messaging, storage, API endpoints, and a full React UI. What's missing is the **social layer**: roles, permissions, and an invitation flow that lets users accept or decline before joining.

This document covers **Phase 1** (roles + invitations) and outlines **Phase 2** (group management polish). A future document will cover communities (federation-style, multi-channel groups).

## Current State

### What works

- MLS group creation, member add/remove, encrypt/decrypt, epoch tracking, state persistence
- Sled-backed storage for group messages, metadata, plaintext cache
- HTTP API: create, invite, send/fetch messages, leave, delete, list members, remove member, reactions
- GossipSub broadcast for group messages, P2P sync for catch-up
- Frontend: create dialog, sidebar items with unread/typing, chat view with member sidebar, manage panel, message bubbles with reactions

### What's missing

1. **No roles or ownership** — `GroupRole` (Admin/Moderator/Member) and `Group.admin_did` exist in the proto but are never set or checked. Any member can kick any other member.
2. **No invitation flow** — Invitations auto-join the recipient via MLS Welcome. No way to accept or decline.
3. **No group metadata editing** — Only name is set at creation. `description`, `avatar_cid`, `admin_did` are proto fields that are never populated.
4. **`GroupInvitation` proto is defined but unused** — The actual invite is sent as a DM with metadata.
5. **`GroupMember` fields are unpopulated** — `role`, `joined_at`, `nickname` are defined but never written.

### Proto definitions already in place

```protobuf
enum GroupRole {
  GROUP_ROLE_UNSPECIFIED = 0;
  GROUP_ROLE_MEMBER = 1;
  GROUP_ROLE_MODERATOR = 2;
  GROUP_ROLE_ADMIN = 3;
}

message GroupMember {
  string did = 1;
  GroupRole role = 2;
  int64 joined_at = 3;
  optional string nickname = 4;
}

message Group {
  string id = 1;
  string name = 2;
  string admin_did = 3;
  repeated GroupMember members = 4;
  int64 created_at = 6;
  optional string avatar_cid = 7;
  optional string description = 8;
}

message GroupInvitation {
  string group_id = 1;
  string group_name = 2;
  string inviter_did = 3;
  string invitee_did = 4;
  int64 timestamp = 6;
  repeated GroupMember members = 8;
  bytes mls_welcome = 10;
  bytes mls_commit = 11;
}
```

No proto changes are needed for Phase 1.

---

## Phase 1: Roles + Permissions + Invitation Flow

### Design Decision: Two-Phase MLS Commits

When a user invites someone, we do **not** immediately merge the MLS commit. Instead:

1. Sender calls `add_members()` on the MLS group — group enters `PendingCommit` state
2. Sender stores the commit + welcome locally as a pending outbound invite
3. Sender sends `GroupInvitation` proto to invitee via encrypted Olm DM
4. Invitee sees invitation in a dedicated **Invitations tab** — can accept or decline
5. **Accept**: invitee processes the Welcome, sends `mls_invite_accepted` DM back to sender. Sender calls `merge_pending_commit()` and broadcasts commit to GossipSub.
6. **Decline**: invitee sends `mls_invite_declined` DM. Sender calls `clear_pending_commit()` to rollback.

**Trade-off**: While an invite is pending, the group is blocked from other MLS operations (no new invites, no encrypted messages). A **5-minute timeout** auto-cancels stale invites to prevent indefinite blocking.

This avoids split-brain states where the sender's epoch is ahead of other members.

### Part A: Backend — Roles & Permissions

#### `crates/variance-messaging/src/mls.rs`

New methods on `MlsGroupHandler`:

| Method | Purpose |
|--------|---------|
| `add_member_deferred(group_id, key_package)` | Calls `add_members()` but does NOT merge. Returns `AddMemberResult`. Group enters `PendingCommit`. |
| `confirm_add_member(group_id)` | Calls `merge_pending_commit()`. Used when invitee accepts. |
| `cancel_add_member(group_id)` | Calls `clear_pending_commit()`. Used on decline or timeout. |

Keep existing `add_member()` (immediate merge) for internal use if needed, or replace entirely.

#### `crates/variance-messaging/src/storage/`

**New file: `storage/invitations.rs`**

New sled trees:
- `pending_invitations` — incoming invitations (invitee side), keyed by `group_id`, stores `GroupInvitation` proto
- `outbound_invites` — sent invitations (sender side), keyed by `group_id`, stores commit bytes + metadata for timeout tracking

New trait methods in `trait_def.rs`:

```rust
// Invitee side
async fn store_pending_invitation(&self, invitation: &GroupInvitation) -> Result<()>;
async fn fetch_pending_invitations(&self) -> Result<Vec<GroupInvitation>>;
async fn fetch_pending_invitation(&self, group_id: &str) -> Result<Option<GroupInvitation>>;
async fn delete_pending_invitation(&self, group_id: &str) -> Result<()>;

// Sender side
async fn store_outbound_invite(&self, group_id: &str, data: &OutboundInvite) -> Result<()>;
async fn fetch_outbound_invite(&self, group_id: &str) -> Result<Option<OutboundInvite>>;
async fn delete_outbound_invite(&self, group_id: &str) -> Result<()>;
async fn fetch_expired_outbound_invites(&self, max_age: Duration) -> Result<Vec<String>>; // returns group_ids
```

**Changes to existing storage:**

When creating a group, populate:
- `Group.admin_did = local_did`
- `Group.created_at = now`
- Add creator as `GroupMember { did: local_did, role: ADMIN, joined_at: now }`

When joining via Welcome, add self as:
- `GroupMember { did: local_did, role: MEMBER, joined_at: now }`

New method for role updates:
```rust
async fn update_group_member_role(&self, group_id: &str, member_did: &str, role: GroupRole) -> Result<()>;
```

#### `crates/variance-app/src/api/groups.rs`

**New permission helper:**

```rust
/// Check that the caller has at least `minimum_role` in the group.
/// Returns the caller's actual role, or 403 Forbidden.
fn require_role(state: &AppState, group_id: &str, minimum_role: GroupRole) -> Result<GroupRole>;
```

**Permission gates:**

| Endpoint | Required Role |
|----------|--------------|
| `mls_invite_to_group` | Admin |
| `mls_remove_member` | Admin (can't remove equal/higher role) |
| `mls_delete_group` | Admin (creator only — matches `admin_did`) |
| `mls_leave_group` | Any member |
| Send message | Any member |

**New endpoints:**

| Route | Method | Purpose |
|-------|--------|---------|
| `/mls/invitations` | GET | List pending incoming invitations |
| `/mls/invitations/{group_id}/accept` | POST | Accept invitation — process Welcome, notify sender |
| `/mls/invitations/{group_id}/decline` | POST | Decline invitation — delete, notify sender |

**Updated responses:**

- `GroupMemberInfo` adds `role: String` ("admin", "moderator", "member")
- `MlsGroupInfo` adds `your_role: String` and `admin_did: String`

#### `crates/variance-app/src/event_router/messaging.rs`

**Changed behavior for MLS Welcome DMs:**

Currently: `handle_mls_welcome_dm()` auto-joins the group.

New behavior:
1. Parse the DM metadata as `GroupInvitation`
2. Store as pending invitation via `store_pending_invitation()`
3. Broadcast `WsMessage::GroupInvitationReceived` to WebSocket clients
4. Do NOT process the Welcome or subscribe to GossipSub yet

**New DM metadata handlers:**

| Metadata type | Handler |
|---------------|---------|
| `mls_invite_accepted` | Sender side: call `confirm_add_member()`, broadcast commit to GossipSub, persist MLS state, delete outbound invite |
| `mls_invite_declined` | Sender side: call `cancel_add_member()`, persist MLS state, delete outbound invite |

**Timeout background task:**

A `tokio::spawn` task that runs every 60 seconds, checks `fetch_expired_outbound_invites(Duration::from_secs(300))`, and calls `cancel_add_member()` + cleanup for each expired invite.

#### `crates/variance-app/src/websocket.rs`

New `WsMessage` variants:

```rust
GroupInvitationReceived {
    group_id: String,
    group_name: String,
    inviter: String,
    inviter_display_name: Option<String>,
    member_count: usize,
},
GroupInvitationAccepted {
    group_id: String,
    member_did: String,
},
GroupInvitationDeclined {
    group_id: String,
},
GroupMemberRoleChanged {
    group_id: String,
    member_did: String,
    new_role: String,
},
```

All four are `MessageCategory::Messages` for subscription filtering.

### Part B: Frontend

#### Types (`app/src/api/types.ts`)

```typescript
interface GroupInvitation {
  group_id: string;
  group_name: string;
  inviter: string;
  inviter_display_name: string | null;
  member_count: number;
  timestamp: number;
}

// Updated:
interface MlsGroupInfo {
  id: string;
  name: string;
  member_count: number;
  last_message_timestamp: number | null;
  has_unread?: boolean;
  your_role: string;    // new
  admin_did: string;    // new
}

interface GroupMemberInfo {
  did: string;
  display_name: string | null;
  role: string;         // new
}

// New WsEvent variants:
| { type: "GroupInvitationReceived"; group_id: string; group_name: string; inviter: string; inviter_display_name: string | null; member_count: number }
| { type: "GroupInvitationAccepted"; group_id: string; member_did: string }
| { type: "GroupInvitationDeclined"; group_id: string }
| { type: "GroupMemberRoleChanged"; group_id: string; member_did: string; new_role: string }
```

#### API Client (`app/src/api/client.ts`)

```typescript
invitationsApi = {
  list:    () => GET  /mls/invitations,
  accept:  (groupId: string) => POST /mls/invitations/{groupId}/accept,
  decline: (groupId: string) => POST /mls/invitations/{groupId}/decline,
}
```

#### Store (`app/src/stores/messagingStore.ts`)

New state:
```typescript
pendingInvitations: GroupInvitation[];
addInvitation: (inv: GroupInvitation) => void;
removeInvitation: (groupId: string) => void;
```

#### New Component: `app/src/components/conversations/InvitationsTab.tsx`

- Dedicated section in the sidebar (tab or collapsible section above conversation list)
- Badge count for pending invitations
- Each invitation shows: group name, inviter display name, member count, timestamp
- Accept / Decline buttons per invitation
- Accept navigates to the new group after joining

#### Updated Components

**`ManageGroupPanel.tsx`**
- Show role badge next to each member name (Admin / Member)
- "Kick" button: only visible if current user is Admin AND target is lower role
- "Delete Group" button: only visible if current user is the admin (creator)
- "Invite" input: only visible if current user is Admin

**`GroupHeader.tsx`**
- Show current user's role as a subtle badge

**`useWebSocket.ts`**
- `GroupInvitationReceived` → add to `pendingInvitations` store, fire OS notification
- `GroupInvitationAccepted` → invalidate `["groups"]` and `["mls/groups/{id}/members"]`
- `GroupInvitationDeclined` → invalidate queries, optionally toast
- `GroupMemberRoleChanged` → invalidate member list queries

---

## Phase 2: Group Management UX (Future)

After Phase 1 ships and is tested:

- **Rename group** — Admin+ only, new endpoint + broadcast name change to members
- **Set description** — Admin+ only
- **Promote/demote members** — Admin can promote Member to Moderator, or demote. Only creator can promote to Admin.
- **Transfer ownership** — Creator can transfer `admin_did` to another Admin
- **Moderator role** — Can invite and kick Members, but not other Moderators or Admins
- **Group avatar** — IPFS CID, Admin+ only

---

## File Change Summary

### New files (~2)
- `crates/variance-messaging/src/storage/invitations.rs`
- `app/src/components/conversations/InvitationsTab.tsx`

### Modified Rust files (~8)
- `crates/variance-messaging/src/mls.rs` — deferred add/confirm/cancel methods
- `crates/variance-messaging/src/storage/trait_def.rs` — new trait methods
- `crates/variance-messaging/src/storage/mod.rs` — delegation for new methods
- `crates/variance-app/src/api/groups.rs` — permission gates, new invitation endpoints
- `crates/variance-app/src/event_router/messaging.rs` — pending invite flow, accept/decline handlers, timeout task
- `crates/variance-app/src/websocket.rs` — new WsMessage variants
- `crates/variance-app/src/state.rs` — any new AppState fields if needed
- `crates/variance-app/src/api/mod.rs` — register new routes

### Modified frontend files (~7)
- `app/src/api/types.ts` — new types, updated interfaces
- `app/src/api/client.ts` — invitationsApi
- `app/src/stores/messagingStore.ts` — pending invitations state
- `app/src/components/conversations/ManageGroupPanel.tsx` — role-aware UI
- `app/src/components/messages/GroupHeader.tsx` — role badge
- `app/src/hooks/useWebSocket.ts` — new event handlers
- Sidebar component (wherever the invitations tab is placed)

### Proto changes: 0
Everything needed is already defined in `messaging.proto`.
