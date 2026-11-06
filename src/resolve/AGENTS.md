# src/resolve/

Relative manifest dependencies resolve from the immutable authored root, not
the later materialized package root. Selection runs after the graph settles;
reapplying filters in a consumer can produce a different result.
