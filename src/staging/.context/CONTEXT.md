# Prepared package materialization

`RootedSourceRef.authored_root` remains the origin for relative dependencies;
`package_root` is the materialized content view. Consumer skill overlays run
after schema preparation and use the installed name after dependency renames.
