# Probe contracts

Pi compatibility and model-listing success are independent. Pi can print help,
version and models on stderr, so use it when stdout is empty.

A failed refresh retains last-good support evidence, but a failed latest gated
listing no longer implies authentication. `--no-refresh-models` can use stale
cache; `MARS_OFFLINE` suppresses probes before cache access.

Cursor's default effort may be an unsuffixed slug. Resolve it against the probe
catalog and clear effort after baking it into the launch model ID.
