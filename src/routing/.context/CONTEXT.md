# Routing contracts

Model preview and launch-bundle share candidate evaluation to prevent divergent
routes. Permission and caller exclusions apply before support or auth probes.

Auth and support are separate: a failed latest gated listing can retain last-good
support without proving auth. Supported authored preferences may win immediately;
otherwise the first unverified route is deferred while eligible routes are tried.
Build policy owns traversal across model attempts.

Serialize `RouteDecisionReport`, not the internal trace: its string labels keep
the public JSON independent of internal enum changes. Route facts belong in the
report, while warnings describe actionable degraded states.
