# Migrating a package to schema v1

Mars 0.34.0 and later read only packages that declare [schema v1](schema-v1.md), and the `mars migrate` command is gone. Support for the format before schema 1 ended in 0.34.0.

To convert a package that still has no `schema` in its `[package]` table, run `mars migrate` from Mars 0.33.x in the package's repository (0.33.7 is the last release that has it), review the result with `git diff`, then upgrade. The 0.33.x guide is [migrating-to-schema-v1.md at v0.33.7](https://github.com/haowjy/mars-agents/blob/v0.33.7/docs/config/migrating-to-schema-v1.md).

A key Mars 0.34.0 no longer reads is a `field.retired` warning, and each one is listed with what replaced it in [Retired keys](retired-keys.md). A package without `schema` is the error `manifest.schema-missing` ([Retired keys](retired-keys.md#package-without-schema)).

## What it changes

The table of what `mars migrate` rewrote is in the 0.33.7 guide linked above. The keys that changed are now listed in [Retired keys](retired-keys.md).

## It edits minimally

This was a property of `mars migrate` in 0.33.x. Mars 0.34.0 has no command that rewrites a package's definitions.
