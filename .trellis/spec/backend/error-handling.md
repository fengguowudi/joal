# Error Handling

> Exception taxonomy, propagation, and logging rules for JOAL.

---

## Taxonomy: checked vs unchecked

JOAL uses a deliberate split between checked and unchecked exceptions:

| Type                                      | Base                | Meaning                                                                 | Caller must handle?   |
|-------------------------------------------|---------------------|-------------------------------------------------------------------------|-----------------------|
| `NoMoreTorrentsFileAvailableException`    | `Exception`         | The torrent pool is exhausted — a recoverable business outcome          | Yes (checked)         |
| `NoMoreUriAvailableException`             | `Exception`         | All tracker URIs for a torrent have failed — recoverable                | Yes (checked)         |
| `TooManyAnnouncesFailedInARowException`   | `Exception`         | An announcer has failed N consecutive times — recoverable at the pipeline level | Yes (checked)  |
| `AppConfigurationIntegrityException`      | `RuntimeException`  | Config validation violation — programmer / operator error              | No (unchecked)        |
| `TorrentClientConfigIntegrityException`   | `RuntimeException`  | Emulated-client config validation violation                             | No (unchecked)        |

**Rule**: domain events a caller can reasonably recover from → checked `Exception`. Programmer/operator errors (bad config, invariants violated) → `RuntimeException`.

### Skeleton for a new exception

Every custom exception in JOAL follows the same minimal shape (see `core/exception/NoMoreTorrentsFileAvailableException.java:1`):

```java
package org.araymond.joal.core.exception;

public class NoMoreTorrentsFileAvailableException extends Exception {
    private static final long serialVersionUID = -2114301657174632211L;

    public NoMoreTorrentsFileAvailableException(final String message) {
        super(message);
    }
}
```

Checklist:
- Always declare `serialVersionUID` (checkstyle/IDE warnings are disabled, so this is a convention, not a tool check).
- Single `String message` constructor is the default. Add more constructors only when a caller actually needs them (e.g. `TooManyAnnouncesFailedInARowException` takes a `MockedTorrent`).
- Package-private constructor is fine when only code in the same package should create the exception (see `core/config/AppConfigurationIntegrityException.java:9`).
- File lives next to the feature that throws it (see `directory-structure.md` → "Exceptions live inside the feature package").

---

## Validation in constructors

Configuration POJOs validate in their constructor and throw `RuntimeException`s. The caller gets a complete or no object — never a half-valid one.

```java
// core/config/AppConfiguration.java:44
private void validate() {
    if (minUploadRate < 0) {
        throw new AppConfigurationIntegrityException("minUploadRate must be at least 0");
    }
    if (maxUploadRate < 0) {
        throw new AppConfigurationIntegrityException("maxUploadRate must greater or equal to 0");
    } else if (maxUploadRate < minUploadRate) {
        throw new AppConfigurationIntegrityException("maxUploadRate must be greater or equal to minUploadRate");
    }
    // ...
}
```

For null/precondition checks inside methods, use Guava `Preconditions`, not hand-written `if` + throw:

```java
// core/torrent/watcher/TorrentFileProvider.java:132
Preconditions.checkNotNull(unwantedTorrents, "unwantedTorrents cannot be null");
```

---

## Catch patterns

### The three canonical catch shapes in this codebase

**1. Catch + log + wrap as `IllegalStateException`** — used when an I/O failure means the component cannot continue. The wrapped exception keeps the cause chain.

```java
// core/config/JoalConfigProvider.java:67
try {
    conf = objectMapper.readValue(joalConfFile.toFile(), AppConfiguration.class);
} catch (final IOException e) {
    log.error("Failed to read configuration file", e);
    throw new IllegalStateException(e);
}
```

**2. Catch + log + fire an event** — used when a failure should be reported to the UI but must not crash the caller:

```java
// core/SeedManager.java:177
try {
    MockedTorrent.fromBytes(bytes);
    // ... write file ...
} catch (final Exception e) {
    log.warn("Failed to save torrent file", e);
    final String errorMessage = firstNonNull(e.getMessage(), "Empty/bad file");
    this.appEventPublisher.publishEvent(new FailedToAddTorrentFileEvent(name, errorMessage));
}
```

Note the `firstNonNull(e.getMessage(), "Empty/bad file")` — NPEs on empty files have no message; provide a fallback.

**3. Catch-all in a thread entry point** — required for handlers invoked by external threads that must not die:

```java
// core/torrent/watcher/TorrentFileProvider.java:100
@Override
public void onFileCreate(final File file) {
    log.info("Torrent file addition detected, hot creating file [{}]", file.getAbsolutePath());
    try {
        final MockedTorrent torrent = MockedTorrent.fromFile(file);
        // ... register torrent ...
    } catch (final IOException | NoSuchAlgorithmException e) {
        log.warn("Failed to read file [{}], moved to archive folder: {}", file.getAbsolutePath(), e);
        this.moveToArchiveFolder(file);
    } catch (final Exception e) {
        // This thread MUST NOT crash. we need handle any other exception
        log.error("Unexpected exception was caught for file [{}], moved to archive folder: {}", file.getAbsolutePath(), e);
        this.moveToArchiveFolder(file);
    }
}
```

The comment `This thread MUST NOT crash` is a load-bearing invariant — the commons-io watcher thread doesn't restart itself.

### Announce failure counter — the domain-specific pattern

The announcer counts consecutive failures and escalates only after a threshold (`core/ttorrent/client/announcer/Announcer.java:80`):

```java
} catch (final Exception e) {
    this.consecutiveFails++;
    if (this.consecutiveFails >= 5) {  // TODO: move to config
        log.warn("[{}] has failed to announce {} times in a row", ...);
        throw new TooManyAnnouncesFailedInARowException(torrent);
    } else {
        log.info("[{}] has failed to announce {}. time", ...);
    }
    throw e;
}
```

Transient failures are logged at `info` and rethrown (the pipeline handles retry/back-off). Persistent failures escalate to a domain exception.

---

## Propagating errors to the UI

There is no REST layer; errors are surfaced to the UI via:

1. **Events**: publish `FailedToAddTorrentFileEvent`, `FailedToAnnounceEvent`, `TooManyAnnouncesFailedEvent`. A matching `web/services/corelistener/Web<Family>EventListener` converts the event into a STOMP payload.
2. **Direct STOMP send** on the controller when inside a `@MessageMapping`:

```java
// web/resources/WebSocketController.java:62
@MessageMapping("/config/save")
public void saveNewConf(final ConfigIncomingMessage message) {
    try {
        seedManager.saveNewConfiguration(message.toAppConfiguration());
    } catch (final Exception e) {
        log.warn("Failed to save conf {}", message.toString(), e);
        messageSendingTemplate.convertAndSend("/config", new InvalidConfigPayload(e));
    }
}
```

Do not let exceptions escape a `@MessageMapping` method unhandled — the WebSocket broker would silently drop them and the UI would be stuck waiting.

---

## Anti-patterns

- `throw new RuntimeException(e)` — use `IllegalStateException(e)` (the project default) or a domain-specific subclass.
- Swallowing with `catch (Exception e) { /* nothing */ }` — always at minimum `log.warn("context {}", ..., e)`.
- `catch (Throwable t)` — no place in this codebase uses it; don't introduce it.
- Logging the exception and then rethrowing a different one without chaining the cause — always pass `e` as the cause (`new IllegalStateException(e)`) so stack traces remain useful.
- Letting a filesystem-watcher handler (`onFileCreate`, etc.) throw — the comment in `TorrentFileProvider.onFileCreate` says it all.
- Declaring custom exceptions without `serialVersionUID`.
- Returning `null` from a public API to signal "failure" — prefer `Optional.empty()` (`Announcer.getLastKnownSeeders()` returns `Optional<Integer>`) or throw.
- Validating `AppConfiguration` outside its constructor — constructor-time validation is the contract callers rely on.
