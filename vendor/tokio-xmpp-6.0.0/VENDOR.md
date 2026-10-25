# tokio-xmpp 6.0.0, pinned local source

Original crates.io package retained as upstream-6.0.0.crate, imported from the
existing local Cargo cache on 2026-10-24. SHA-256 from the previous lockfile:
8921a165ab1eefd447cb1661423c582ac77b9c6960e123a71c8ad9cebc40b026.
Registry: https://crates.io/crates/tokio-xmpp/6.0.0 . Upstream repository:
https://gitlab.com/xmpp-rs/xmpp-rs (as recorded by the package).

All original files, manifests, examples and upstream copyright notices are
retained except Cargo's generated cache markers. Local source changes,
including the standalone test lockfile and local parser override, are fully
recorded in patches/0001-durable-ingress.patch. Reproduce the complete source
with tools/check-tokio-xmpp-vendor.py. No Livewire patch was copied.

## Current local changes

- A synchronous InboundCommit callback is run on the sole stream worker before
  further socket I/O or frontend delivery. Failure drops the transport without
  footer or reconnect; no error payload is logged. Snapshot publication must
  follow successful commit. Pre-SM stanzas and final parser dispositions mean
  callback count must not be interpreted as protocol h.
- CheckpointCommit receives the worker's actual IngressContext and a parsed
  stanza or a final parser-discard marker. Ready context includes real SM
  counters and a non-consuming snapshot of both outbound queue layers.
  Pre-negotiation traffic is marked not-ready, preventing provisional state
  from replacing a resumable checkpoint. Invalid counted stanzas reach this
  barrier before acks, even though no application event is delivered.
- Owned quiet-detach captures the worker's SM state, server metadata, inbound
  h, last outbound ack base, unacked stanzas and both layers of unsent work.
  Buffered frontend events have already committed; draining them prevents
  backpressure deadlocks. Debug hides token, location and stanza contents.
- Library-owned c2s login/backoff tasks are tracked. After the stream worker
  has stopped, quiet-detach aborts and joins all these tasks before returning.
  Generic callbacks passed directly to new_with_handoff/new_with_ingress
  still own their externally spawned tasks and must manage them themselves.
- From-handoff constructors restore SM state and outbound work. The caller
  enforces server expiry and the original authenticated full JID. Remote h
  is checked against the restored ack base and unacked queue; already accepted
  stanzas are removed, remaining ones replay before newer unsent traffic.
- A resumed event carries bound JID/features into a new ClientWorker with no
  Reset cache. It never unwraps absent cached state. A wrong resumed token
  produces a protocol error. Internal SM Debug no longer prints the token.
- Resume failure or missing/no-resume SM support preserves the remaining
  outbound queue for fresh bind, rather than dropping it. Fresh replay is
  at-least-once and requires application deduplication. The local parser patch
  accepts failed without optional h. No-SM delivery cannot promise server ack.
- ClientWorker forwards EOF; clean cleanup tolerates an already ended worker.
- Split stream/client receivers now register an AtomicWaker when a concurrent
  writer holds the shared mutex. Writer/metadata guard release unlocks first,
  then wakes the receiver. A retry after registration closes the release race.
  No lock futures or reservations survive poll cancellation/reunite, and idle
  receiver polls do not self-wake. Two deterministic regression tests fail on
  the original try_lock/Pending path; the fix and latest-waker behavior pass.

Both production and standalone tests use the pinned local xmpp-parsers source.
The standalone library lockfile otherwise preserves its original dependency
pins; daemon tests exercise the application's exact pins. The daemon runtime now supplies CheckpointCommit, consumes only persisted
inbox rows, restores saved transport state and uses quiet-detach at shutdown.
Contextual ingress is atomically persisted with the format-2 checkpoint in
transport_handoff.rs and tested at the real SM ack boundary. Inbox completion
can join the application's message/ratchet transaction. The production inbox dispatcher is wired; full UI/push/Postal lifecycle
budgets and general outbound write-ahead remain to be finished. This is not a
claim of completed store delivery, outbound write-ahead or stock validation.

## Licence

Cargo.toml declares MPL-2.0. Original copyright and MPL notices remain and
local modifications are dated and remain MPL-2.0. LICENSE is the full MPL
text already present in the project's generated bundle at import. No source
is marked with the MPL Exhibit B incompatibility notice. Puhuri's own code
remains AGPL-3.0-or-later. The modified source, patch and build inputs must be
available with the corresponding source before any future binary publication.
This work does not authorize publication.
