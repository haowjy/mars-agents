# Staging

Staging projects schema-prepared packages into scratch space; schema classification and validation belong to `mars-schema`. It copies the binary-owned hook, MCP, bootstrap and plugin-manifest surfaces as well as prepared content, while preserving the authored root as the source origin.

Preparation diagnostics gate materialization; later diagnostics gate publication. Real sync publishes a staged generation by rename under the sync lock. Dry runs use temporary space outside the project and publish nothing. Consumer skill overlays run after preparation and use the installed name after renaming.
