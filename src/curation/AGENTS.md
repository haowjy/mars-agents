# Model curation

Curation controls the model list display; it must not affect routing, build, alias resolution or the underlying possible rows.

Rules fold from user to project to local. A higher-tier glob show cannot undo an effective lower-tier hide; a literal show can. `inherit = false` discards lower tiers. Only a literal show for a concrete harness can declare a row absent from the possible inventory, and out-of-scope declarations are discarded.
