# Samply.Spot v0.2.5 2026-06-24

## Changes

* `--cors-origin` / `CORS_ORIGIN` is now optional. If unset, Spot runs without adding CORS headers.
* Updated `reqwest` from `0.12` to `0.13`.
* Replaced `async-sse` with `sse-stream` for consuming Beam task results.

# Samply.Spot v0.2.4 2025-10-24

## Breaking changes

* Project parameter is now mandatory