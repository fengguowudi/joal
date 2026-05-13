# Journal - fengguowudi (Part 1)

> AI development session journal
> Started: 2026-05-10

---



## Session 1: Bootstrap JOAL backend spec guidelines

**Date**: 2026-05-10
**Task**: Bootstrap JOAL backend spec guidelines
**Branch**: `master`

### Summary

Initialized Trellis framework + AI platform configs (.claude/.codex/.cursor), then filled all 5 backend spec files with JOAL-specific conventions (Java 11, Spring Boot 2.7.3, Log4j2, Lombok, javax.inject, JUnit 5 + Mockito + AssertJ) anchored to real source paths. Repurposed database-guidelines as 'Persistence (No Database)' since JOAL has no JPA/JDBC. Archived 00-bootstrap-guidelines task.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `fa72ab4` | (see git log) |
| `b5dc7b9` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 2: Wrap up MVP-1 Rust engine

**Date**: 2026-05-12
**Task**: Wrap up MVP-1 Rust engine
**Branch**: `master`

### Summary

Finished MVP-1 headless Rust engine for JOAL. trellis-check verified all 10 PRD acceptance items (166 tests, clippy/fmt clean) and fixed one DRY issue in torrent/watcher.rs by extracting rename_with_overwrite. Committed the watcher refactor; separately committed a chore(tooling) bundle syncing Trellis scripts/version + platform hooks + a batch of third-party .claude skills (caveman/diagnose/grill-with-docs/improve-codebase-architecture/prototype/review/tdd). Cleaned up the unrelated skills-main/ vendor dir. Parent task stays in_progress for MVP-2 egui frontend next round.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `4e9f0f0` | (see git log) |
| `2f1640e` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 3: Cleanup duplicate helpers in key/peer_id generators

**Date**: 2026-05-13
**Task**: Cleanup duplicate helpers in key/peer_id generators
**Branch**: `master`

### Summary

Extracted shared helpers (lock_state, default_shared_state, compile_rand_regex, string_from_ascii_regex_bytes, TORRENT_PERSISTENT_TTL, TimedState, AccessAwareEntry) from key.rs and peer_id.rs into generator/common.rs. Net -71 lines, zero behavior change, 168 tests green.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `660ecde` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete
