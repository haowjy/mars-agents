# Harness probes

Pi compatibility and model listing are separate observations. A failed listing does not invalidate a successful help-surface check; without last-good model evidence it leaves support unknown rather than proving the model absent. Some Pi builds print version, help and model listings to stderr, so its probe falls back to stderr when stdout is empty.

The Pi help check covers the spawn surface Meridian needs; a missing required token group marks Pi incompatible before launch. Each group admits alternative spellings to accommodate Pi versions. An empty successful model listing is evidence of no runnable models, unlike a failed listing.

Cursor represents default effort tiers with an unsuffixed model slug. Resolve effort against the listing and fail closed if no matching slug exists; on success the launch bundle carries the chosen slug and clears the separate effort field.

A failed refresh retains last-good support evidence, but a failed latest Pi or Cursor listing does not prove current authentication. Background refresh can leave the current command using its prior observation. A command must keep the timestamp and error associated with the observation it loaded rather than rereading the cache later.

If the refresh lock is held, use a usable cached result without writing; without one, a command may probe for itself but must not persist that result. This avoids racing the lock holder.

Tests that depend on probe-cache state must set `MARS_CACHE_DIR` to a temporary directory: XDG cache variables do not isolate Windows tests.
