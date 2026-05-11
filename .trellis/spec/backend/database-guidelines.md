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
- Trigger: `rust/crates/joal-core/src/client/**` now both parses JOAL `clients/*.client` files and executes the runtime refresh semantics encoded in those files.
- Why code-spec depth is mandatory: `.client` is a persisted cross-module contract shared by config loading, announce query construction, and tracker-visible peer-id/key behavior.

### 2. Signatures
- `BitTorrentClientConfig::try_from(&str) -> Result<BitTorrentClientConfig, ClientError>`
- `BitTorrentClientConfig::validate(&self) -> Result<(), ClientError>`
- `UrlEncoder::encode(&self, &str) -> Result<String, ClientError>`
- `UrlEncoder::encode_bytes(&self, &[u8]) -> Result<String, ClientError>`
- `NumwantProvider::get(&self, RequestEvent) -> i32`
- `PeerIdGenerator::get(&self, &InfoHash, RequestEvent) -> Result<String, ClientError>`
- `KeyGenerator::get(&self, &InfoHash, RequestEvent) -> Result<String, ClientError>`

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
  - runtime identity methods are torrent-aware: the cache key is `InfoHash`, not only `RequestEvent`
  - `NEVER`: generate once per generator instance, then reuse forever
  - `TIMED`: reuse current value until `refreshEvery` seconds have elapsed, then rotate
  - `TORRENT_VOLATILE`: cache per `InfoHash`; on `RequestEvent::Stopped`, return the current value first and then evict that torrent entry
  - `TORRENT_PERSISTENT`: cache per `InfoHash`; evict entries after 120 minutes of inactivity
  - peer-id `TORRENT_PERSISTENT` mirrors Java's lazy sweep cadence: run eviction every 30 `get(...)` calls, not on every call
  - key `TORRENT_PERSISTENT` mirrors Java's eager sweep cadence: run eviction on every `get(...)`
  - key `TIMED_OR_AFTER_STARTED_ANNOUNCE`: return the current key for `STARTED`, then rotate so the next call sees the new key
  - peer-id generation must still enforce the 20-byte invariant before any value becomes tracker-visible
  - historical S4/S5 boundary rule still applies for future staged work: before a refresh policy is implemented correctly, parsing/validation must fail fast instead of silently emulating the wrong behavior
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
| `TIMED` / `TIMED_OR_AFTER_STARTED_ANNOUNCE` has `refreshEvery < 1` | integrity error during config load |
| peer-id algorithm produces anything other than 20 bytes | integrity error before returning a tracker-visible value |
| `requestHeaders` fields are present but empty strings | allow; Java only enforces non-null, not non-empty |
| a future staged port adds a refresh policy shell before the runtime semantics | fail fast; never silently coerce that policy into `ALWAYS` |

### 5. Good / Base / Bad Cases
- Good: `resources/clients/qbittorrent-4.5.0.client` parses unchanged, retains Java field names/casing, and yields `KeyGenerator::TORRENT_PERSISTENT`.
- Good: `TIMED_OR_AFTER_STARTED_ANNOUNCE` returns the current key for `STARTED`, then rotates so the next announce uses a new key.
- Base: a minimal `.client` file with `peerIdGenerator`, `urlEncoder`, one request header, and a `query` without `{key}` parses without a `keyGenerator`.
- Base: `TORRENT_VOLATILE` returns the same value for repeated announces of the same torrent until a `STOPPED` announce, while a different `InfoHash` gets its own cached value.
- Bad: a `.client` file that uses `{key}` in `query` but omits `keyGenerator` must fail validation.
- Bad: treating `NEVER`, `TIMED`, `TORRENT_PERSISTENT`, or `TORRENT_VOLATILE` as if they were equivalent to `ALWAYS`.
- Bad: evicting `TORRENT_VOLATILE` entries before returning the `STOPPED` value, or rotating the `STARTED` key before returning it.

### 6. Tests Required
- Golden test: parse `resources/clients/*.client` and assert all repository fixtures deserialize successfully.
- Focused golden test: assert `qbittorrent-4.5.0.client` field-by-field, including serde rename/casing and the `TORRENT_PERSISTENT` key policy.
- Unit test: exhaustive `0x00..=0xFF` coverage for `UrlEncoder` with a real exclusion pattern.
- Unit test: invalid regex and invalid algorithm payloads fail during config parsing/validation.
- Unit test: `NEVER` reuses the same peer-id/key across multiple torrents and events.
- Unit test: `TIMED` reuses until the threshold, then rotates after the threshold.
- Unit test: `TORRENT_VOLATILE` caches per `InfoHash` and evicts only after returning the `STOPPED` value.
- Unit test: peer-id `TORRENT_PERSISTENT` evicts stale entries only on the 30th `get(...)` sweep boundary.
- Unit test: key `TORRENT_PERSISTENT` evicts stale entries on the next `get(...)`.
- Unit test: `TIMED_OR_AFTER_STARTED_ANNOUNCE` returns the pre-rotation key on `STARTED` and the rotated key on the following call.

### 7. Wrong vs Correct
#### Wrong
```rust
if event == RequestEvent::Started {
    state.key = Some(generate_key(algorithm, *key_case)?);
}
state.key.clone().unwrap()
```

#### Correct
```rust
let key = state.key.clone().expect("key initialized");
if event == RequestEvent::Started {
    state.key = Some(generate_key(algorithm, *key_case)?);
}
Ok(key)
```

The Java contract is observable at the tracker boundary: `STARTED` must use the current key, and only the following announce may see the rotated one.

---

## Scenario: Rust announcer + orchestrator

### 1. Scope / Trigger
- Trigger: `rust/crates/joal-core/src/announcer/**` (tracker HTTP client + stateful per-torrent `Announcer`) and `rust/crates/joal-core/src/ttorrent_client/**` (DelayQueue-driven orchestrator) are now the authoritative announce pipeline for the Rust port.
- Why code-spec depth is mandatory: the announce URL is tracker-visible and the orchestrator sequences persistence side-effects (archive) with bandwidth bookkeeping (register/unregister). A silent regression in either breaks byte-level compatibility with `ttorrent-core 1.5` or corrupts the upload/weight accounting used by the bandwidth dispatcher.

### 2. Signatures
- `SuccessAnnounceResponse::parse_with_uri(bytes: &[u8], uri: &Uri) -> Result<SuccessAnnounceResponse, AnnouncerError>`
- `TrackerClient::announce(&self, query: String, headers: HeaderMap) -> Result<SuccessAnnounceResponse, AnnouncerError>`
- `Announcer::announce(&self, event: RequestEvent) -> Result<SuccessAnnounceResponse, AnnouncerError>`
- `DelayQueue::<T: InfoHashed>::add_or_replace(&self, item: T, delay: Duration)`
- `DelayQueue::drain_all(&self) -> Vec<T>`
- `ClientOrchestrator::stop(&self)` — drains the queue and emits only valid terminal announces.
- `TorrentFileProvider::move_to_archive_folder(&self, info_hash: &InfoHash)` — filesystem move, never delete.

### 3. Contracts
- **Announce URL separator**: the final URL is `base_uri + sep + query`, where `sep = '?'` if `base_uri` contains no `?`, otherwise `'&'` (Java `TrackerClient.makeCallAndGetResponseAsByteBuffer`; Rust `announcer/tracker.rs`). Trackers that embed pre-existing query parameters in the announce URL must see those parameters preserved.
- **Self-exclusion clamp on seeders**: `seeders = max(0, complete - 1)` on every `SuccessAnnounceResponse::parse_with_uri`. The `-1` subtracts the client itself from the seeder count the tracker reports (Java `SuccessAnnounceResponse`; Rust `announcer/response.rs`). Downstream weight calculation in `bandwidth/weight.rs` assumes this clamp has already been applied.
- **`failure reason` is a typed error, not a success**: a BEP-3 response of the form `d14:failure reason...e` must promote to `AnnouncerError::TrackerReported` even when the HTTP status is 200. It is the only case where a 200 OK body is not a successful announce (Rust `announcer/response.rs` + `announcer/error.rs`).
- **Consecutive-failure threshold**: `Announcer::announce` increments `consecutive_fails` on every error path and escalates to `AnnouncerError::TooManyFailures` when `consecutive_fails >= 5` (Java `Announcer.announce` constant; Rust `announcer/state.rs`). The constant is fixed, not configurable.
- **Stop-phase drain rule**: when `ClientOrchestrator::stop()` drains the `DelayQueue`, entries whose `event == RequestEvent::Started` are **dropped**, not converted to `Stop`. Rationale: a torrent with a pending STARTED has never announced, so the tracker has no session to tear down. Sending a STOP for an unknown info-hash is either silently ignored (good tracker) or a protocol violation (strict tracker). (Java `Client.stop()` stream filter; Rust `ttorrent_client/client.rs::stop`.)
- **Archive, never delete** (extends the existing persistence contract to the Rust watcher): `TorrentFileProvider::move_to_archive_folder` is the only sanctioned disposal path. Failed / removed / malformed / ratio-met torrents all move to `<confDirRoot>/torrents/archived/`. Watcher-side parse errors on newly-created `.torrent` files also archive instead of propagating, matching Java `TorrentFileProvider.onFileCreate` (`core/torrent/watcher/TorrentFileProvider.java:109`).
- **Response handler chain order (fixed)**: `AnnounceEventPublisher → AnnounceReEnqueuer → BandwidthDispatcherNotifier → ClientNotifier`. This order is load-bearing:
  - `AnnounceReEnqueuer` schedules the next tick before any side-effects run, so a handler panicking downstream still leaves a valid queue entry.
  - `BandwidthDispatcherNotifier` must run **before** `ClientNotifier`. If `ClientNotifier` archives a torrent first, the bandwidth dispatcher is left holding a dangling `InfoHash` weight entry until the next weight recompute, skewing per-torrent allocation.
- **DelayQueue dedup key = `InfoHash`**: `add_or_replace(item, delay)` removes any existing entry with the same info-hash before inserting the new one. Prevents double-announce under rapid event churn (e.g., add → remove → add of the same `.torrent` file); the newer event supersedes the older one rather than queueing both.

### 4. Validation & Error Matrix

| Condition | Expected result |
|-----------|-----------------|
| Tracker returns HTTP 200 with a body containing `failure reason` | `AnnouncerError::TrackerReported`, counts as a failure |
| Tracker returns HTTP 200 but bencode is missing `interval` / `complete` / `incomplete` | `AnnouncerError::IncompleteResponse`, counts as a failure |
| Tracker reports `complete = 0` | `seeders = 0` (clamp), not a negative number |
| Transport error (connect refused / timeout) on the current URI | rotate to next URI via `TrackerClientUriProvider` before failing the call |
| All URIs exhausted in one pass | `AnnouncerError::NoMoreUri`, counts as a failure |
| 5th failure in a row (any failure mode) | `AnnouncerError::TooManyFailures`, orchestrator must archive + pull next torrent |
| `ClientOrchestrator::stop()` finds a pending `RequestEvent::Started` in the queue | drop, do not convert to `Stop` |
| `.torrent` file parse fails in the watcher (new file added) | move the offending file to `torrents/archived/`, do not propagate the error |
| `DelayQueue::add_or_replace` called with an info-hash already present | replace in place, do not queue both |

### 5. Good / Base / Bad Cases
- Good: announce URL `http://t.example/announce?passkey=abc` with template query `info_hash=...&event=started` yields `http://t.example/announce?passkey=abc&info_hash=...&event=started` — pre-existing `?passkey` survives.
- Good: tracker returns `d8:completei10e10:incompletei5e8:intervali1800ee` → `SuccessAnnounceResponse { seeders: 9, leechers: 5, interval: 1800 }`. The `9` is the clamp.
- Good: orchestrator processes a STARTED announce, receives `interval: 1800`, re-enqueues the same info-hash as a regular event 1800 s later. A concurrent `.torrent` file deletion supersedes that regular entry with a STOP at delay 1 s.
- Base: a fresh `ClientOrchestrator::start` loads three `.torrent` files, schedules three STARTED announces at delay 0. The DelayQueue holds exactly three distinct info-hashes.
- Base: a stop-phase drain with one STARTED pending and one regular pending emits exactly one STOP (for the regular entry) and drops the STARTED.
- Bad: promoting a tracker `failure reason` response into a `SuccessAnnounceResponse` because the HTTP layer returned 200. The caller will record a phantom `interval` and re-schedule a doomed announce.
- Bad: unregistering a torrent from the bandwidth dispatcher **after** the client archives it. The weight calculator can observe a zero-file/non-zero-weight moment between the two side-effects.
- Bad: calling `Files::remove_file` on a `.torrent` instead of `move_to_archive_folder` — users lose the recovery trail.

### 6. Tests Required
- Unit: `SuccessAnnounceResponse::parse_with_uri` on `d8:completei1e10:incompletei0e8:intervali1800ee` asserts `seeders == 0` (clamp proof).
- Unit: `parse_with_uri` on `d14:failure reason12:access denyede` returns `AnnouncerError::TrackerReported`.
- Unit: `TrackerClient::announce` with a URI provider of `[refused_tcp_port, wiremock_ok]` rotates to the second URI and returns success — covered by `tests/announcer_http.rs::rotates_after_transport_failure`.
- Unit: `Announcer::announce` fails five times in a row and on the fifth returns `TooManyFailures`.
- Unit: `DelayQueue::add_or_replace` with duplicate info-hash and shorter delay demotes the earlier entry; `drain_all` returns only the later one.
- Integration: `tests/orchestrator_end_to_end.rs` — start orchestrator with a `.torrent` file, wiremock tracker returns `interval: 30`; assert the first request carries `event=started`; call `stop()`; assert a `event=stopped` is sent before the orchestrator exits.
- Integration: dropping a malformed `.torrent` file into the watched directory archives the file instead of killing the watcher task.

### 7. Wrong vs Correct
#### Wrong
```rust
// Bandwidth unregister runs after archive in the handler chain.
pub fn handler_chain() -> AnnounceResponseHandlerChain {
    chain.register(AnnounceEventPublisher::new(bus.clone()));
    chain.register(AnnounceReEnqueuer::new(queue.clone()));
    chain.register(ClientNotifier::new(orchestrator.clone()));           // archives torrent
    chain.register(BandwidthDispatcherNotifier::new(dispatcher.clone())); // unregisters weight
    chain
}

// Stop drain converts STARTED → STOP.
for request in queue.drain_all() {
    announcer_executor.execute(request.to_stop());
}
```

#### Correct
```rust
// BandwidthDispatcherNotifier must see the event before ClientNotifier can archive the torrent.
pub fn handler_chain() -> AnnounceResponseHandlerChain {
    chain.register(AnnounceEventPublisher::new(bus.clone()));
    chain.register(AnnounceReEnqueuer::new(queue.clone()));
    chain.register(BandwidthDispatcherNotifier::new(dispatcher.clone()));
    chain.register(ClientNotifier::new(orchestrator.clone()));
    chain
}

// Stop drain drops pending STARTED entries; only already-announced torrents get a STOP.
for request in queue.drain_all() {
    if request.event() == RequestEvent::Started {
        continue; // tracker has no session for this info-hash yet
    }
    announcer_executor.execute(request.to_stop());
}
```

The handler order is a silent correctness invariant: reordering compiles and passes any test that doesn't assert on the observable "bandwidth weight freed before torrent archived" ordering. Lock it down with a direct chain-composition test in `ClientBuilder::build`.

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
