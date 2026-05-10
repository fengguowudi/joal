# Quality Guidelines

> Code standards, Lombok conventions, dependency injection style, testing baseline.

---

## Language level and build

- Java 11 (see `pom.xml:32` — `<java.version>11</java.version>` + `maven-compiler-plugin` with matching source/target).
- Build: `./mvnw clean verify` or `mvn clean verify`. Tests run under `spring-boot-starter-test` (JUnit 5 + Mockito + AssertJ + Spring test).
- No checkstyle/spotbugs/PMD plugin is wired. Reviewers enforce the conventions below by hand; stay consistent.

---

## Dependency injection

Constructor injection only. Use `javax.inject.@Inject`, not Spring's `@Autowired`:

```java
// web/resources/WebSocketController.java:48
@Inject
public WebSocketController(final SeedManager seedManager, final JoalMessageSendingTemplate messageSendingTemplate) {
    this.seedManager = seedManager;
    this.messageSendingTemplate = messageSendingTemplate;
}
```

Fields: `private final`. Never reassign a dependency after construction.

Two places deviate: `SeedManager` itself is instantiated manually (it is created pre-context by `ApplicationReadyListener`-adjacent code), and `BitTorrentClientProvider` uses a manual `Provider<AppConfiguration>`. Follow the `@Inject` constructor pattern everywhere else.

---

## Lombok conventions

The project uses Lombok heavily. Match these exact usages — don't invent variants.

| Annotation                    | When                                                                  | Example                                                 |
|-------------------------------|-----------------------------------------------------------------------|---------------------------------------------------------|
| `@Slf4j`                      | Any class that logs                                                   | `core/SeedManager.java:56`                              |
| `@Getter` (class-level)       | All fields need getters (value objects, event POJOs)                  | `core/config/AppConfiguration.java:15`                  |
| `@Getter` (field-level)       | Only some fields need getters                                         | `core/ttorrent/client/announcer/Announcer.java:30`      |
| `@RequiredArgsConstructor`    | All `final` fields fed by the constructor — pair with `@Getter` for event POJOs | `core/events/announce/SuccessfullyAnnounceEvent.java:8` |
| `@EqualsAndHashCode`          | Value objects where equality is by all fields                         | `core/config/AppConfiguration.java:14`                  |

### What to avoid

- `@Data` — it auto-generates setters, which violates the immutability convention. None of the existing POJOs use it.
- `@Builder` on event/value POJOs — current code uses direct constructors (`new SuccessfullyAnnounceEvent(a, RequestEvent.STARTED)`).
- `@AllArgsConstructor` / `@NoArgsConstructor` — prefer `@RequiredArgsConstructor` with explicit `final` fields.
- `val` / `var` (Lombok's versions) — code consistently uses `final` with explicit types.

Fields intended to be non-final (e.g. stateful counters in `Announcer`) are declared as plain non-final fields; no annotation marks them.

---

## `final` everywhere

- Fields: `private final` unless the class genuinely needs mutable state (`Announcer.consecutiveFails`, `SeedManager.seeding`).
- Parameters: `final` on public and private methods alike (see every example file in this spec).
- Local variables: `final` when they aren't reassigned.

This is a style invariant across the codebase — diffs that drop `final` will stand out in review.

---

## Async and threading

- `@EnableAsync` is on `JackOfAllTradesApplication`. The `TaskExecutor` bean lives in `conf/SpringConf.java`:

```java
// conf/SpringConf.java:14
@Bean
public TaskExecutor taskExecutor() {
    final ThreadPoolTaskExecutor executor = new ThreadPoolTaskExecutor();
    executor.setCorePoolSize(5);
    executor.setMaxPoolSize(10);
    executor.setQueueCapacity(25);
    return executor;
}
```

- Event listeners that do non-trivial work use `@Async` + `@EventListener`:

```java
// core/CoreEventListener.java:20
@Async
@Order(Ordered.HIGHEST_PRECEDENCE)
@EventListener
public void handleTorrentFileAddedForSeed(final TorrentFileAddedEvent event) {
    log.debug("Event TorrentFileAddedEvent caught");
}
```

- Handlers called from external threads (`TorrentFileProvider.onFileCreate`, announcer retry loops) **must not throw**. See `error-handling.md` → catch shape #3.

---

## Event-driven state propagation

- State changes publish a `core/events/<family>/<Name>Event.java` via `ApplicationEventPublisher.publishEvent(...)`.
- Listeners live in `core/CoreEventListener` (internal bookkeeping) and `web/services/corelistener/Web<Family>EventListener` (UI fanout).
- Listeners MUST NOT call back into `SeedManager`'s mutation methods. Quote from `CoreEventListener.java:14`:

  > They MUST NOT interact with JOAL state, otherwise this class will soon turn into a god damn mess...

Treat events as one-way. If a listener needs new behavior, publish a new event rather than calling mutators.

---

## Testing

### Framework stack

JUnit 5 (`org.junit.jupiter.api.Test`) + Mockito + AssertJ. See `pom.xml:129`. Spring security tests use `spring-security-test`.

### Naming

- Class: `<ClassUnderTest>Test.java`, mirroring the package. Web-app integration tests use the `*WebAppTest.java` suffix (`web/config/EndpointObfuscatorConfigurationWebAppTest.java`).
- Methods: `shouldDoXWhenY` (or just `shouldDoX`). Use `public void`, no parameters.

### Assertions — AssertJ only

```java
// core/config/AppConfigurationTest.java:54
@Test
public void shouldNotBuildIfMaxRateIsLesserThanMinRate() {
    assertThatThrownBy(() -> new AppConfiguration(180L, 179L, 2, "azureus.client", false, 1f))
            .isInstanceOf(AppConfigurationIntegrityException.class)
            .hasMessageContaining("maxUploadRate must be greater or equal to minUploadRate");
}
```

- `assertThat(value).isEqualTo(...)`, `.isInstanceOf(...)`, `.usingRecursiveComparison()`.
- `assertThatThrownBy(() -> ...)` for exception assertions.
- Never use JUnit `Assertions.assertEquals` / `assertThrows` — AssertJ is the project standard.

### Mocks and spies

Mockito via `org.mockito.Mockito` (the long form, not `org.mockito.BDDMockito`). `ArgumentCaptor` for event verification:

```java
// core/config/JoalConfigProviderTest.java:87
@Test
public void shouldPublishConfigHasBeenLoadedEventOnConfigLoad() throws FileNotFoundException {
    final ApplicationEventPublisher publisher = Mockito.mock(ApplicationEventPublisher.class);
    final JoalConfigProvider provider = new JoalConfigProvider(new ObjectMapper(), joalFoldersPath, publisher);

    final AppConfiguration loadedConf = provider.loadConfiguration();

    final ArgumentCaptor<ConfigHasBeenLoadedEvent> captor = ArgumentCaptor.forClass(ConfigHasBeenLoadedEvent.class);
    Mockito.verify(publisher, Mockito.times(1)).publishEvent(captor.capture());

    final ConfigHasBeenLoadedEvent event = captor.getValue();
    assertThat(event.getConfiguration()).isEqualTo(loadedConf);
}
```

### Test fixtures

- Realistic paths: `Paths.get("src/test/resources/configtest")`. Don't mock filesystem access; use the real fixtures.
- For rewritable fixtures, always clean up in `finally` (`JoalConfigProviderTest.shouldWriteConfigurationFile`).
- Reuse `core/utils/MockedInjections.java` and `core/utils/TorrentFileCreator.java` instead of re-building mocks inline.

### Coverage baseline

- Every public class under `core/` has a matching `*Test.java`. Keep it that way — a PR that adds a new `core/` class without tests stands out.
- `@VisibleForTesting` is allowed (Guava) to broaden visibility for tests — see `JoalConfigProvider.loadConfiguration` (`core/config/JoalConfigProvider.java:61`).

---

## Immutability and nullability

- Prefer `Optional<T>` return types for "maybe present" getters (`Announcer.getLastKnownLeechers()` → `Optional<Integer>`). Don't return `null`.
- Pre-check arguments with `Preconditions.checkNotNull(arg, "message")` from Guava, not Spring's `Assert`.
- Immutable collection helpers: `Collections.emptyList()`, `Collections.emptyMap()` (already statically imported in `SeedManager`).

---

## Anti-patterns

- Field injection (`@Autowired private Foo foo;`) — constructor injection with `@Inject` is the only allowed form.
- Writing to `System.out` / `System.err` — use `@Slf4j` + the right level.
- Introducing `@Data` on new POJOs — use explicit `@Getter @RequiredArgsConstructor` (+ `@EqualsAndHashCode` when equality matters).
- Mutable setters on config/event/value objects.
- JUnit 4 (`@RunWith`, `junit.framework.*`) — project is on JUnit 5.
- Hamcrest / JUnit assertions — AssertJ only.
- Adding a new exception without `serialVersionUID` (see `error-handling.md`).
- Large `@SpringBootTest` when a plain unit test would do — `@SpringBootTest` is reserved for the `*WebAppTest.java` files that genuinely exercise the web stack.
