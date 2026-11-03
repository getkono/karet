# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.0](https://github.com/getkono/karet/compare/karet-jsonrpc-v0.6.1...karet-jsonrpc-v0.7.0) - 2026-09-24

### Added

- *(jsonrpc)* [**breaking**] answer a cancelled peer request with the caller's error
- *(jsonrpc)* let a consumer answer peer requests at its own pace
- *(jsonrpc)* let a caller await a connection's death

### Fixed

- *(jsonrpc)* log a deferred reply the stopped writer can no longer take
- *(jsonrpc)* drain deferred replies through one task per connection
- honour dynamic registration, and close the review's mapping gaps
- *(jsonrpc)* report a peer's null-id rejection instead of dropping it

### Other

- put drain_stderr's comment back and narrow what close() logs
- *(jsonrpc)* pin that close() writes replies deferred before it first
- *(jsonrpc)* document how close() orders against peer replies
- *(jsonrpc)* say the reply backlog is unbounded and when a reply is lost
