# ADR-006: Native project file format

**Status:** Accepted (Phase 0)

## Decision
- A `.rusting` file is UTF-8 JSON: `{ "format_version": 1, "document": { ... } }`.
- Loading: parse the envelope, reject versions newer than supported, migrate older ones (a `migrate(version, json)` hook), deserialize, then `validate()` the references. Errors are typed (`FileFormatError::{Io, Malformed, UnsupportedVersion, Invalid}`) and never panic.
- Maps are `BTreeMap`, so output is deterministic and diff-friendly.
- Changing the serialized shape means bumping `FORMAT_VERSION` and adding a migration step.

## Later
A zip container with a JSON manifest plus separate per-drawing assets (and raster tiles) once files grow large. Unsupported data must be kept on round trip or reported, never dropped silently.
