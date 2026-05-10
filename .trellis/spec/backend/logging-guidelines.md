# Logging Guidelines

> Log4j2 via the SLF4J facade. Lombok `@Slf4j` on every class that logs.

---

## Library and bootstrapping

- `spring-boot-starter-logging` (logback) is **excluded** from the Spring Boot starter; `spring-boot-starter-log4j2` replaces it (see `pom.xml:42`).
- Configuration: `src/main/resources/log4j2.xml`. It defines one `CONSOLE` appender with a highlighted pattern and sets `org.araymond.joal` to `INFO`.
- `shutdownHook="disable"` — JOAL stops logging via its own `ApplicationClosingListener`. Don't re-enable the default hook.

Pattern (for reference when parsing logs):

```
[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%15t] %c{1.}: %msg%n%throwable
```

---

## How to declare a logger

Always use Lombok `@Slf4j`. Do not declare loggers by hand:

```java
// core/SeedManager.java:56
@Slf4j
public class SeedManager {
    // use log.info(...), log.warn(...), etc.
}
```

Compiled result is an SLF4J `Logger` named after the class — identical for every class in the codebase.

---

## Log levels — when to use which

This is how existing code uses levels. Match these, not a generic SLF4J guide.

| Level   | When                                                                                                         | Example                                                                                   |
|---------|--------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------|
| `error` | Unrecoverable I/O, invariant broken, something requiring operator attention                                  | `JoalConfigProvider.java:68` — failed to read `config.json`                               |
| `warn`  | Recoverable problem the user should know about (bad torrent file, failed save, missing optional folder)      | `SeedManager.java:178` — failed to save torrent; `SeedManager.java:263` — missing conf dir |
| `info`  | Lifecycle + expected per-torrent events (announce success, torrent added/removed, proxy configured)          | `Announcer.java:70` — successful announce; `TorrentFileProvider.java:101` — torrent added |
| `debug` | Internal bookkeeping, event listener callbacks, configuration load trace                                     | `CoreEventListener.java:24` — event caught; `JoalConfigProvider.java:43` — conf path      |
| `trace` | Not used in this codebase — don't introduce it                                                               | —                                                                                         |

Debug logs inside hot paths should be guarded with `log.isDebugEnabled()` only when the argument construction is expensive (see `web/resources/WebSocketController.java:64`). For plain parameterised messages, skip the guard.

---

## Message format — parameterised, not concatenated

Use `{}` placeholders. Never `+`-concatenate.

```java
// core/torrent/watcher/TorrentFileProvider.java:101
log.info("Torrent file addition detected, hot creating file [{}]", file.getAbsolutePath());

// core/ttorrent/client/announcer/Announcer.java:70
log.info("{} has announced successfully. Response: {} seeders, {} leechers, {}s interval",
        this.torrent.getTorrentInfoHash().getHumanReadable(),
        responseMessage.getSeeders(),
        responseMessage.getLeechers(),
        responseMessage.getInterval());
```

### Conventions seen in the codebase

- Wrap identifiers with square brackets: `[{}]` for file paths and infohashes — makes logs grep-friendly.
- Use `InfoHash.getHumanReadable()` when logging a torrent; never log the raw infohash bytes.
- For multi-line "section" logs (e.g. one-off startup banners) the existing code uses rows of `=` signs — see `SeedManager.java:101`. Don't add this style to new high-frequency logs.

---

## Logging exceptions

The exception is always the **last** argument of the call — SLF4J routes it to the appender's `%throwable` field:

```java
// core/config/JoalConfigProvider.java:68
log.error("Failed to read configuration file", e);

// core/torrent/watcher/TorrentFileProvider.java:107
log.warn("Failed to read file [{}], moved to archive folder: {}", file.getAbsolutePath(), e);
```

Do not do `log.error("failed: " + e.getMessage())` — you lose the stack trace.

Pattern for fire-and-fanout error handling (catch → log → publish event):

```java
// core/SeedManager.java:177
} catch (final Exception e) {
    log.warn("Failed to save torrent file", e);
    final String errorMessage = firstNonNull(e.getMessage(), "Empty/bad file");
    this.appEventPublisher.publishEvent(new FailedToAddTorrentFileEvent(name, errorMessage));
}
```

---

## What to log

- Component lifecycle: startup, shutdown, start-seeding, stop-seeding.
- Per-torrent transitions: added, removed, archived, announce success/fail, retry count.
- Configuration changes: load path, save result, dirty state.
- Unexpected branches in watcher/announcer threads (these threads MUST NOT die silently — see `error-handling.md`).

## What NOT to log

- `info` or above for per-packet / per-tick events — bandwidth dispatcher ticks, delay-queue pops. These either stay at `debug` or aren't logged at all.
- Raw request queries or tracker responses at `info` — these can include peer IDs and user-agent values that emulate specific clients. Keep them at `debug` if you really need them.
- Contents of `config.json` — it's an operator-facing file and may contain tracker-sensitive tuning.
- Full torrent payloads — at most `MockedTorrent.getName()` and `InfoHash.getHumanReadable()`.

---

## Anti-patterns

- `private static final Logger log = LogManager.getLogger(Foo.class);` — redundant with `@Slf4j`.
- Mixing `System.out.println` / `System.err.println` into production code.
- `log.error("...", e.getMessage())` instead of `log.error("...", e)` — drops the stack trace.
- String concatenation (`log.info("x=" + x)`) — bypasses the parameterised-logging optimisation and reads worse.
- `log.trace(...)` — not used here; introduce it only with a reason and a companion `log4j2.xml` level change.
- Swallowing exceptions silently or logging them at `debug` — the three catch shapes in `error-handling.md` set the expected level per case.
