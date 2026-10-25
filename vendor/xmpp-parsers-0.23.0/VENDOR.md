# xmpp-parsers 0.23.0, pinned source

Imported 2026-10-24 from the local Cargo registry cache. Original package:
https://crates.io/crates/xmpp-parsers/0.23.0 . Upstream repository recorded in
Cargo metadata: https://gitlab.com/xmpp-rs/xmpp-rs . Archive retained as
upstream-0.23.0.crate, SHA-256 from the former daemon/Cargo.lock:
92b5d63bbd122ce3b5287d83efdd6916fb88bb274be23f6a9140238e0e1465b0.

All original files are retained, excluding generated Cargo cache markers.
The local MPL-2.0 patch changes Failed.h from attribute to attribute(default),
so a server's SM resume rejection without h parses as h=None. The field is
already Option<u32> upstream. No network or parsing failure is treated as an
ack; the tokio-xmpp fallback retains all unacknowledged stanzas when h is absent.
The real absent-h XML path is covered by tokio-xmpp's handoff protocol test.

Cargo.toml declares MPL-2.0 and original file copyright/MPL notices remain.
The full LICENSE text is retained from the project's existing generated MPL
bundle. Local changes remain MPL-2.0, carry a dated modification notice and
are fully reproduced by patches/0001-sm-failed-optional-h.patch.
No Livewire patch was copied. tools/check-tokio-xmpp-vendor.py verifies both
pinned archives, applies their patches and compares every original source file.
The daemon and standalone tokio-xmpp tests both select this path override.
The source/patch/full licence must accompany the corresponding source release
before any future binary distribution. This import does not publish anything.
