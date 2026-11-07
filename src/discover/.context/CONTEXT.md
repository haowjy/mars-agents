# Discovery

`mars-schema` prepares agents and skills; binary discovery adds hooks and bootstrap documents. Do not introduce another agent or skill tree walk here.

Hooks are direct children of the package-root `hooks/`; bootstrap discovery uses the schema's bounded walk and selects the shallowest layer containing bootstrap documents. Duplicate names of the same discovered kind fail rather than silently shadowing each other.

The walk excludes hidden directories and generated plugin trees so local execution surfaces cannot become package inputs. A foreign hidden layout remains importable by explicitly rooting a dependency at it. The configured plugin output directory is excluded even before its sentinel exists. `.mars-src` definitions override matching current-package definitions before staging.
