# Directory Structure

> Where code lives in JOAL (Jack-of-All-Trades), a Java 11 / Spring Boot 2.7.3 BitTorrent seeder.

---

## Top-level layout

```
src/main/java/org/araymond/joal/
├── JackOfAllTradesApplication.java   # Spring Boot entry (@EnableAsync, excludes DispatcherServlet + ErrorMvc)
├── ApplicationReadyListener.java     # ApplicationReadyEvent handler (start-time hooks)
├── ApplicationClosingListener.java   # ContextClosedEvent handler (shutdown hooks)
├── conf/                             # Spring @Configuration beans (TaskExecutor, etc.)
├── core/                             # Business logic — MUST NOT depend on web/
└── web/                              # STOMP WebSocket transport — adapts core/ to the UI

src/main/resources/
├── application.properties            # Spring Boot config (random port, undertow tuning, disable JMX)
├── log4j2.xml                        # Log4j2 config (spring-boot-starter-logging is excluded in pom.xml)
└── public/                           # React SPA served statically
```

Entry point: `src/main/java/org/araymond/joal/JackOfAllTradesApplication.java:11`.

---

## `core/` — the domain

`core/` is where the business logic lives. It does not depend on `web/`. The outer boundary is `SeedManager` — everything torrent-related flows through it (see the class Javadoc at `src/main/java/org/araymond/joal/core/SeedManager.java:52`).

```
core/
├── SeedManager.java                  # Facade — all torrent/config/bandwidth ops go through here
├── CoreEventListener.java            # Internal @Async @EventListener (global state bookkeeping)
├── bandwith/                         # NOTE: package is spelled "bandwith" (not "bandwidth") — DO NOT rename
│   └── weight/                       # PeersAwareWeightCalculator, WeightHolder
├── client/emulated/                  # BitTorrent client identity emulation (qBittorrent, azureus, ...)
│   └── generator/{key,peerid,numwant}/   # One generator family per announce field, with Algorithm sub-packages
├── config/                           # AppConfiguration (JSON-backed via config.json)
├── events/                           # Spring ApplicationEvent POJOs — NO behavior, just data
│   ├── announce/                     # SuccessfullyAnnounceEvent, FailedToAnnounceEvent, WillAnnounceEvent, TooManyAnnouncesFailedEvent
│   ├── config/                       # ConfigHasBeenLoadedEvent, ConfigurationIsInDirtyStateEvent, ListOfClientFilesEvent
│   ├── global/state/                 # GlobalSeedStartedEvent, GlobalSeedStoppedEvent
│   ├── speed/                        # SeedingSpeedsHasChangedEvent
│   └── torrent/files/                # TorrentFileAddedEvent, TorrentFileDeletedEvent, FailedToAddTorrentFileEvent
├── exception/                        # Cross-package domain exceptions
├── torrent/
│   ├── torrent/                      # MockedTorrent, InfoHash (value objects)
│   └── watcher/                      # TorrentFileWatcher, TorrentFileProvider (filesystem hot-reload)
└── ttorrent/client/                  # ttorrent protocol client (announcer pipeline)
    └── announcer/
        ├── Announcer.java, AnnouncerFacade.java, AnnouncerFactory.java
        ├── exceptions/               # TooManyAnnouncesFailedInARowException
        ├── request/                  # AnnounceRequest, AnnounceDataAccessor, AnnouncerExecutor, SuccessAnnounceResponse
        ├── response/                 # Chain of AnnounceResponseHandler implementations (ClientNotifier, BandwidthDispatcherNotifier, AnnounceEventPublisher, AnnounceReEnqueuer)
        └── tracker/                  # TrackerClient, TrackerClientUriProvider, TrackerResponseHandler, NoMoreUriAvailableException
```

### Feature-based packaging, not layer-based

`core/` uses **feature packages** (`bandwith/`, `torrent/`, `client/emulated/`, `ttorrent/client/announcer/{request,response,tracker}/`). Do not introduce `service/`, `dto/`, or `model/` horizontal folders inside `core/` — they conflict with the existing pattern.

Exceptions live **inside the feature package that throws them**, not in a global bag:

- `core/ttorrent/client/announcer/tracker/NoMoreUriAvailableException.java`
- `core/ttorrent/client/announcer/exceptions/TooManyAnnouncesFailedInARowException.java`
- `core/config/AppConfigurationIntegrityException.java` (package-private constructor — must stay next to `AppConfiguration`)

Only truly cross-package exceptions go in `core/exception/` (today only `NoMoreTorrentsFileAvailableException`).

---

## `web/` — STOMP WebSocket transport

`web/` adapts user-facing WebSocket messages to `SeedManager` calls. `web/` depends on `core/`, never the reverse.

```
web/
├── annotations/                      # @ConditionalOnWebUi custom annotation
├── config/                           # Spring @Configuration for WebSocket/MVC/Jackson
│   ├── BeanConfig.java, WebMvcConfiguration.java, WebSocketConfig.java, JacksonConfig.java
│   ├── obfuscation/                  # AbortNonPrefixedRequestFilter + EndpointObfuscatorConfiguration (URL prefix guard)
│   └── security/                     # WebSecurityConfig, WebSocket auth & authorization
│       └── websocket/{interceptor,services}/
├── messages/
│   ├── incoming/config/              # Wire → AppConfiguration (ConfigIncomingMessage, Base64TorrentIncomingMessage)
│   └── outgoing/
│       ├── StompMessage.java         # Envelope wrapping all outbound payloads
│       └── impl/{announce,config,files,global/state,speed}/   # Mirror of core/events/
├── resources/
│   └── WebSocketController.java      # @Controller with @MessageMapping / @SubscribeMapping — no @RestController
└── services/
    ├── JoalMessageSendingTemplate.java
    └── corelistener/                 # One @Component per event family (WebAnnounceEventListener, WebConfigEventListener, ...)
```

### Events → outgoing payloads: the mirror pattern

Every `core/events/<family>/XxxEvent.java` has a matching `web/messages/outgoing/impl/<family>/XxxPayload.java`. Fanout is done by `web/services/corelistener/Web<Family>EventListener.java`:

```
core/events/torrent/files/TorrentFileAddedEvent.java
  ↓ published by SeedManager / TorrentFileProvider
  ↓ two listeners subscribe:
    core/CoreEventListener.java                                (internal bookkeeping, @Async)
    web/services/corelistener/WebTorrentFileEventListener.java (fanout to UI)
      ↓ wraps as
    web/messages/outgoing/impl/files/TorrentFileAddedPayload.java
      ↓ sent via
    web/services/JoalMessageSendingTemplate
```

When adding a new event:
1. Create `core/events/<family>/<Name>Event.java` (POJO with `@RequiredArgsConstructor @Getter`).
2. Publish via `appEventPublisher.publishEvent(new <Name>Event(...))`.
3. If the UI needs it, add `web/messages/outgoing/impl/<family>/<Name>Payload.java` and handle it in the matching `Web<Family>EventListener`.

---

## `src/test/java/org/araymond/joal/`

Test tree mirrors the main tree one-to-one. Shared test helpers:

- `TestConstant.java` — top-level test constants.
- `core/utils/MockedInjections.java` — pre-built mocks shared across tests.
- `core/utils/TorrentFileCreator.java` — builds valid `MockedTorrent` instances for tests.
- `springtestconf/MockedSeedManagerBean.java` — test `@Configuration` that replaces `SeedManager` with a mock.

Test resources: `src/test/resources/{configtest,rewritable-config,torrent-store}` — real `config.json` fixtures and torrent file stores.

Web-tier integration tests end in `*WebAppTest.java` (see `web/config/EndpointObfuscatorConfigurationWebAppTest.java`); plain unit tests end in `*Test.java`.

---

## Anti-patterns

- Adding a `service/` or `dto/` package inside `core/` — feature packages are the convention.
- Importing `org.araymond.joal.web.*` from anywhere under `core/` — breaks the core→web one-way dependency.
- Creating `core/models/` or `core/dto/` for event POJOs — events live under `core/events/<family>/` and payloads under `web/messages/outgoing/impl/<family>/`.
- Renaming `core/bandwith` to `bandwidth` — it is intentionally stable across the codebase; a rename touches every import and every historical commit reference.
- Putting a `@RestController` anywhere — this project exposes no REST surface; everything user-facing goes through STOMP `@MessageMapping` in `web/resources/WebSocketController.java`.
- Adding persistent storage (JPA, JDBC, Spring Data). Configuration is `config.json`, torrent files are the filesystem — see `database-guidelines.md`.
