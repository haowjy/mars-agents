# Harness probes

Pi compatibility and model listing are separate observations. A failed listing does not invalidate a successful help-surface check; without last-good model evidence it leaves support unknown rather than proving the model absent. Some Pi builds print version, help and model listings to stderr, so its probe falls back to stderr when stdout is empty.

The Pi help check covers the spawn surface Meridian needs; a missing required token group marks Pi incompatible before launch. Each group admits alternative spellings to accommodate Pi versions. An empty successful model listing is evidence of no runnable models, unlike a failed listing.

Antigravity folds an effort tier into its model slugs (`agy models` lists `gemini-3.8-flash-low`, with no bare slug standing for a level). Its `ProbeSpec` has `SlugShape::Bare`. Meridian prints its command and does not run it; a `--harness agy` pin resolves the model and effort to one listed slug (`models/slug_tiers.rs` finds the family's tiers, `build/policy/runnable.rs` picks), and the tiers are that model's levels for the effort rule. A tiered slug matches no catalog row, so `mars models list` shows no effort levels for it.

`agy models` prints `Fetching available models...` and then `<slug>\t<display name>` per line. Its output when logged out was not observed, so a non-zero exit and an empty list both read as a failed listing (`empty_is_failure`), and a listing that completes is credential evidence.

A failed refresh retains last-good support evidence, but a failed latest Pi or Antigravity listing does not prove current authentication. Background refresh can leave the current command using its prior observation. A command must keep the timestamp and error associated with the observation it loaded rather than rereading the cache later.

One cache implementation (`cache.rs`) serves Pi, OpenCode and Antigravity; the run, timeout and output cleanup of a listing command are shared in `command.rs`; a harness adds a `CachedProbe` impl, not a copy. A probe-backed harness is one `ProbeSpec` row in `harness/registry.rs`: its `ProbeDriver::of::<Result>()`, its `ListingAuth`, and its `SlugShape`. Everything outside the probe modules reads a result as a `Listing` (slugs, whether the listing completed, error, incompatibility, whether an empty listing proves nothing), never as the harness's own result type; `CachedProbe::listing` is where each result says so. Routing's matching for a `Bare` shape lives in `bare_listing_assessment` in `routing/mod.rs`, which calls `models/slug_tiers.rs::find`, the one rule for whether such a listing has a model. Every write, foreground or detached, goes through `persist_probe_attempt`, which keeps the last-good listing and moves `last_attempt_at` past `fetched_at` on failure.

If the refresh lock is held, use a usable cached result without writing; without one, a command may probe for itself but must not persist that result. This avoids racing the lock holder.

Tests that depend on probe-cache state must set `MARS_CACHE_DIR` to a temporary directory: XDG cache variables do not isolate Windows tests.
