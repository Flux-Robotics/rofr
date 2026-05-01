# Changelog

## [unreleased]

- Added idling indefinitely when a cluster is run without any services
  registered.
- Added idling indefinitely when a service doesn't contain any endpoints or
  streams.

## v0.1.1

- Remove the need for `serde_json` as a dependency in implementations.
- Remove the need for `ulid` as a dependency in implementations.
- Remove `schemars` and schema headers.

## v0.1.0

- Initial release.
