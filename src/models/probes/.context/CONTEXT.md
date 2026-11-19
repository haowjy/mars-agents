# Harness probes

Pi compatibility and model listing are separate observations. A failed listing does not invalidate a successful help-surface check; without last-good model evidence it leaves support unknown rather than proving the model absent. Some Pi builds print version, help and model listings to stderr, so its probe falls back to stderr when stdout is empty.

The Pi help check covers the spawn surface Meridian needs; a missing required token group marks Pi incompatible before launch. Each group admits alternative spellings to accommodate Pi versions. An empty successful model listing is evidence of no runnable models, unlike a failed listing.

Cursor folds an effort tier into its model slugs: the `medium` tier is the unsuffixed slug and the others carry a suffix. Any harness can do so by setting `tiers` on its `ProbeSpec` (`SlugTiers`: the level a bare slug stands for, and the harness's own spelling of a model models.dev spells differently); `models/slug_tiers.rs` reads the tiers a model lists, and they are the harness's level set for the effort rule, which moves a level the model lacks to the nearest tier. Fail closed only when the listing is unavailable or has no slug for the model; on success the launch bundle carries the chosen slug and clears the separate effort field.

A failed refresh retains last-good support evidence, but a failed latest Pi or Cursor listing does not prove current authentication. Background refresh can leave the current command using its prior observation. A command must keep the timestamp and error associated with the observation it loaded rather than rereading the cache later.

One cache implementation (`cache.rs`) serves Pi, OpenCode and Cursor; a harness adds a `CachedProbe` impl, not a copy. A probe-backed harness is one `ProbeSpec` row in `harness/registry.rs`: its `ProbeDriver::of::<Result>()`, its `ListingAuth`, its `SlugShape` and its optional `SlugTiers`. Everything outside the probe modules reads a result as a `Listing` (slugs, whether the listing completed, error, incompatibility, whether an empty listing proves nothing), never as the harness's own result type; `CachedProbe::listing` is where each result says so. Routing's matching for a `Bare` shape (Cursor) lives in `bare_listing_assessment` in `routing/mod.rs`, with `models/slug_tiers.rs` for names and prefixes; `cursor.rs` keeps only Cursor's probe, its parsing and its spelling. Every write, foreground or detached, goes through `persist_probe_attempt`, which keeps the last-good listing and moves `last_attempt_at` past `fetched_at` on failure.

If the refresh lock is held, use a usable cached result without writing; without one, a command may probe for itself but must not persist that result. This avoids racing the lock holder.

Tests that depend on probe-cache state must set `MARS_CACHE_DIR` to a temporary directory: XDG cache variables do not isolate Windows tests.
