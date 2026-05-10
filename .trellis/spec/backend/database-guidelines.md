# Persistence Guidelines (No Database)

> JOAL has **no database**. All state lives in the filesystem and in memory. This file documents the actual persistence model so AI sessions don't reach for JPA / JDBC / Spring Data when adding features.

---

## What is persisted

| What                     | Where                                              | Format                | Owner                                                                           |
|--------------------------|----------------------------------------------------|-----------------------|---------------------------------------------------------------------------------|
| App configuration        | `<confDirRoot>/config.json`                        | JSON (Jackson)        | `core/config/JoalConfigProvider.java`                                           |
| Torrent files (active)   | `<confDirRoot>/torrents/*.torrent`                 | BEP-3 bencoded bytes  | `core/torrent/watcher/TorrentFileProvider.java`                                 |
| Torrent files (archived) | `<confDirRoot>/torrents/archived/*.torrent`        | BEP-3 bencoded bytes  | `TorrentFileProvider.moveToArchiveFolder`                                       |
| Emulated client configs  | `<confDirRoot>/clients/*.client`                   | JSON (Jackson)        | `core/client/emulated/BitTorrentClientProvider.java`                            |
| In-flight torrent state  | In-memory only — `Map<File, MockedTorrent>`, etc.  | `MockedTorrent`, `InfoHash`, `Speed` | `TorrentFileProvider`, `BandwidthDispatcher`, `SeedManager`      |

No records, no rows, no migrations. State that should survive a restart lives in `config.json` or on disk as `.torrent`. State that should not survive a restart is just a field on a bean.

The `confDirRoot` layout is resolved once at startup by `SeedManager.JoalFoldersPath` (`src/main/java/org/araymond/joal/core/SeedManager.java:247`).

---

## Reading / writing `config.json`

Do not read `config.json` directly. Go through `JoalConfigProvider`, which caches the parsed `AppConfiguration` and fires Spring events on load/save:

```java
// core/config/JoalConfigProvider.java:61
AppConfiguration loadConfiguration() {
    final AppConfiguration conf;
    try {
        conf = objectMapper.readValue(joalConfFile.toFile(), AppConfiguration.class);
    } catch (final IOException e) {
        log.error("Failed to read configuration file", e);
        throw new IllegalStateException(e);
    }
    this.appEventPublisher.publishEvent(new ConfigHasBeenLoadedEvent(conf));
    return conf;
}
```

On save, the provider emits `ConfigurationIsInDirtyStateEvent` so the UI can react (`core/config/JoalConfigProvider.java:78`).

`AppConfiguration` itself validates in its constructor and throws `AppConfigurationIntegrityException` — never trust raw JSON input (`core/config/AppConfiguration.java:44`).

### Jackson conventions for persisted POJOs

- `@JsonIgnoreProperties(ignoreUnknown = true)` — tolerate older/newer field sets.
- `@JsonCreator` constructor with `@JsonProperty(value = "...", required = true|false)` per field — explicit required flag per field, no reflective field access.
- No setters. Fields are `final`; Lombok `@Getter` exposes them.

See `core/config/AppConfiguration.java:13` for the canonical example.

---

## Scenario: Rust `.client` file compatibility

### 1. Scope / Trigger
- Trigger: `rust/crates/joal-core/src/client/**` now parses JOAL `clients/*.client` files directly, so the file-format contract is no longer "Java-only knowledge".
- Why code-spec depth is mandatory: `.client` is a persisted cross-module contract shared by config loading, announce query construction, and golden tests.

### 2. Signatures
- `BitTorrentClientConfig::try_from(&str) -> Result<BitTorrentClientConfig, ClientError>`
- `BitTorrentClientConfig::validate(&self) -> Result<(), ClientError>`
- `UrlEncoder::encode(&self, &str) -> Result<String, ClientError>`
- `UrlEncoder::encode_bytes(&self, &[u8]) -> Result<String, ClientError>`
- `NumwantProvider::get(&self, RequestEvent) -> i32`

### 3. Contracts
- File location: `<confDirRoot>/clients/*.client`
- File format: JSON with Java field names preserved exactly:
  - `peerIdGenerator`
  - `keyGenerator`
  - `urlEncoder`
  - `query`
  - `requestHeaders`
  - `numwant`
  - `numwantOnStop`
- `urlEncoder` contract:
  - `encodingExclusionPattern: String`
  - `encodedHexCase: "lower" | "upper"`
- `peerIdGenerator` / `keyGenerator` contract:
  - `algorithm` is serde-tagged by `type`
  - refresh policy comes from `refreshOn`
  - S4/S5 boundary rule: before a refresh policy is implemented correctly, parsing/validation must fail fast instead of silently emulating the wrong behavior
- `UrlEncoder` contract:
  - Tracker-visible bytes are encoded byte-by-byte as `%HH`
  - ASCII bytes matching `encodingExclusionPattern` pass through unchanged
  - Non-ASCII bytes are always percent-encoded
  - `encode_bytes()` is the canonical path for raw `info_hash`-style data

### 4. Validation & Error Matrix

| Condition | Expected result |
|-----------|-----------------|
| `.client` JSON is malformed | return `ClientError`, do not build a partial config |
| `query` contains `{key}` but `keyGenerator` is absent | integrity error |
| `urlEncoder.encodingExclusionPattern` is invalid regex | regex validation error during config load |
| peer-id/key algorithm payload is invalid (`length <= 0`, bad regex, invalid pool/checksum config) | validation error during config load |
| refresh policy is present but not implemented for the current stage | explicit fail-fast error, never silently downgrade to `ALWAYS` |
| `requestHeaders` fields are present but empty strings | allow; Java only enforces non-null, not non-empty |

### 5. Good / Base / Bad Cases
- Good: `resources/clients/qbittorrent-4.5.0.client` parses unchanged and retains Java field names/casing.
- Base: a minimal `.client` file with `peerIdGenerator`, `urlEncoder`, one request header, and a `query` without `{key}` parses without a `keyGenerator`.
- Bad: a `.client` file that uses `{key}` in `query` but omits `keyGenerator` must fail validation.
- Bad: treating `NEVER`, `TIMED`, `TORRENT_PERSISTENT`, or `TORRENT_VOLATILE` as if they were already equivalent to `ALWAYS`.

### 6. Tests Required
- Golden test: parse `resources/clients/*.client` and assert all repository fixtures deserialize successfully.
- Focused golden test: assert `qbittorrent-4.5.0.client` field-by-field, including serde rename/casing.
- Unit test: exhaustive `0x00..=0xFF` coverage for `UrlEncoder` with a real exclusion pattern.
- Unit test: invalid regex and invalid algorithm payloads fail during config parsing/validation.
- Unit test: unsupported pre-S5 refresh policies fail fast rather than generating incorrect peer-id/key values.

### 7. Wrong vs Correct
#### Wrong
```rust
// Pretends S5 refresh semantics already exist.
match refresh_on {
    RefreshPolicy::Always => algorithm.generate(),
    RefreshPolicy::Never
    | RefreshPolicy::Timed(_)
    | RefreshPolicy::TorrentPersistent
    | RefreshPolicy::TorrentVolatile => algorithm.generate(),
}
```

#### Correct
```rust
match refresh_on {
    RefreshPolicy::Always => algorithm.generate(),
    other => Err(ClientError::UnsupportedRefreshPolicy(other)),
}
```

This preserves correctness: partial implementations must stop loudly instead of producing tracker-visible behavior that looks valid but diverges from Java.

---

## Writing torrent files to disk

Only one path writes torrent files: `SeedManager.saveTorrentToDisk` (`core/SeedManager.java:171`). It:

1. Calls `MockedTorrent.fromBytes(bytes)` first to validate. If parsing fails, the file is never written.
2. Uses `Files.write(path, bytes, StandardOpenOption.CREATE)` — never `TRUNCATE_EXISTING` and never a blind overwrite.
3. On failure, publishes `FailedToAddTorrentFileEvent` instead of throwing to the caller.

Do not introduce a second write path. Do not call `Files.write` on the torrents directory directly from `web/`.

### Archiving, not deleting

Torrent files are never deleted programmatically. Failed or user-removed torrents are moved to `<confDirRoot>/torrents/archived/` via `TorrentFileProvider.moveToArchiveFolder` (`core/torrent/watcher/TorrentFileProvider.java:144`). The archive directory is created at startup if missing.

### Hot-reload

`TorrentFileProvider` extends `FileAlterationListenerAdaptor` from commons-io. Do not poll the directory yourself — register a `TorrentFileChangeAware` listener via `TorrentFileProvider.registerListener` and receive callbacks.

---

## Concurrency notes

- `TorrentFileProvider.torrentFiles` is a `synchronizedMap(new HashMap<>())`. Iteration inside handlers must still tolerate concurrent modification — the existing handlers copy into a `new ArrayList<>(this.torrentFiles.values())` before exposing data.
- Filesystem events (`onFileCreate` / `onFileChange` / `onFileDelete`) are invoked from the commons-io watcher thread. The watcher-thread handlers in `TorrentFileProvider` must not throw — any other exception is caught and the file is archived (`core/torrent/watcher/TorrentFileProvider.java:109`). Keep that invariant when extending.

---

## Anti-patterns

- Adding `spring-boot-starter-data-jpa`, Hibernate, Flyway, Liquibase, or Spring Data anything. This is a seeder; there is nothing relational to persist.
- Hand-rolling a `FileWatcher` or `WatchService` instead of reusing `TorrentFileProvider`.
- Reading `config.json` directly in a new component — inject `JoalConfigProvider` instead, so the single source of truth and the `ConfigHasBeenLoadedEvent` are preserved.
- Deleting torrent files instead of moving them to `archived/`. Users want the archive as a recovery trail.
- Adding mutable setters to `AppConfiguration` or any persisted POJO — it is rebuilt from JSON and re-validated on every load.
- Writing files with `StandardOpenOption.TRUNCATE_EXISTING` when saving torrents — `CREATE` (fail if exists) is the deliberate default so concurrent uploads don't clobber each other.
