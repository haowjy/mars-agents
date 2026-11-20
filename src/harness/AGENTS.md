# Harness capabilities

Harness identity belongs to `mars-schema`. Keep installation facts separate from routing permission: an excluded harness is not an absent executable, and excluded harnesses must not be probed.

Share one lazy `CapabilitySession` and `NativeAuthCache` across a command's candidate evaluations. Authentication has an unknown state and doesn't prove credit or quota. Pi and Antigravity listings imply configured authentication only while the latest credential-gated listing succeeded; OpenCode's ungated listing never does.

A probe-backed harness is one `ProbeSpec` row in `registry.rs` plus a `CachedProbe` impl in `models/probes/`; the session, routing, listing and `__refresh-probe` find it through the row. Don't add a per-harness field, accessor or `match` on `HarnessId` for listings. A harness whose slugs are flat names with no provider says so with `SlugShape::Bare`, never a comparison on `HarnessId`.

See [probe contracts](../models/probes/.context/CONTEXT.md).
