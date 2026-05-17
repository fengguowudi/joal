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

---

## Rust port — testing conventions

The following rules apply to the Rust workspace under `rust/crates/` (same spec, different language). They supplement, not replace, the Java conventions above.

### Gotcha: `Instant::now().checked_sub(ttl)` is platform-unsafe in tests

**Symptom**: a test panics with `called Option::unwrap() on a None value` only on freshly booted Windows hosts (or any machine whose process uptime is less than the TTL being subtracted). Same test passes reliably on Linux / macOS / long-uptime boxes.

**Cause**: `std::time::Instant` is monotonic and anchored to an unspecified epoch — on Windows it is the boot-time performance counter. Subtracting a `Duration` that is larger than the current anchor value saturates to `None` via `checked_sub`. Tests that simulate an "expired" entry by writing

```rust
entry.last_access = Instant::now().checked_sub(TTL + Duration::from_secs(1)).unwrap();
```

are therefore non-deterministic: they depend on how long the host has been up.

**Fix (convention)**: expose a `#[cfg(test)]`-gated override on the state type and short-circuit the staleness check in test builds only. The field itself does not exist in release builds, so production semantics stay byte-identical.

```rust
// rust/crates/joal-core/src/client/generator/peer_id.rs
struct AccessAwarePeerId {
    value: PeerId,
    last_access: Instant,
    #[cfg(test)]
    force_stale: bool, // only visible to tests — zero cost in release
}

impl AccessAwarePeerId {
    fn should_evict(&self, now: Instant) -> bool {
        #[cfg(test)]
        if self.force_stale {
            return true;
        }
        now.duration_since(self.last_access) > TORRENT_PERSISTENT_TTL
    }

    #[cfg(test)]
    fn mark_stale_for_test(&mut self) {
        self.force_stale = true;
    }
}
```

**Why this over alternatives**:
- Injecting a clock (passing `Instant` in everywhere) bloats every call site for a test-only concern.
- `unwrap_or_else(|| Instant::now())` in the test masks the panic but silently turns a "stale entry" test into a "fresh entry" test — the assertion afterwards would pass vacuously.
- A `#[cfg(test)]` helper keeps production paths untouched and makes the test intent explicit.

**Related**: don't use `Instant::now().checked_sub(...)` anywhere inside `#[test]` code. Grep the diff for it before merging.

### Convention: no `.unwrap()` / `.expect()` outside `#[cfg(test)]`

Applies to every Rust crate in this workspace. The CLI (`joal-app`) uses `anyhow::Context` to bubble errors; library crates (`joal-core`) use `thiserror` — see `error-handling.md`. Tests may freely `unwrap()` on fixtures.

Grep check before committing: `rg '\.unwrap\(\)' rust/crates/ --glob '!tests/**' --glob '!**/tests.rs'` should only hit `#[cfg(test)] mod tests` blocks.

### Convention: egui discarded-pass UI must pin widget ids explicitly

**Symptom**: debug logs emit `Widget rect ... changed id between passes` when a table/filter/config surface changes during a multi-pass frame. In `egui 0.34.x` this typically happens when `Grid` / `TableBuilder` requests a discard pass and the first pass mutates which widgets appear at a given rect.

**Cause**: egui compares widget ids between passes at the same screen rect. Auto-generated ids are fine for static layouts, but dynamic toolbars and row-based tables can reshuffle them when the first pass flips a filter, opens a side panel, or changes the visible row set.

**Fix (convention)**:
- Give interactive inputs explicit ids: `TextEdit::id_salt(...)`, `ComboBox::from_id_salt(...)`, `ui.push_id(...)`.
- For row/slot-based layouts whose visual identity is the screen position (tables, sortable headers, action slots), scope ids by the stable slot key (`row.index()`, column name, toolbar slot), not by domain data that can move to another rect inside the same frame.
- When a small reusable helper (`metric`, `badge`, `chip`, status strip) renders **live data**, the outer `push_id(id, ...)` wrapper is NOT enough — every inner widget whose text mutates frame-to-frame needs its own static-string `push_id` too. Auto-generated child ids derive from the parent counter and the cached rect; once the value text changes (`"0 B/s"` → `"1.2 KB/s"`) the rect width shifts a sub-pixel and the cached id no longer matches.
- `ui.add_sized([ui.available_width(), ...], ...)` is the same trap as `ui.add_sized([fixed, ...], ...)` for multi-pass id stability — the size argument participates in id derivation, and `ui.available_width()` can differ by sub-pixel amounts across passes when sibling columns reflow. Read `ui.available_width()` once, then use `Button::min_size(egui::vec2(min_width, ..))` so egui derives the id from the button itself and lets the layout decide the final width.
- When a togglable surface (config editor, ad-hoc tool panel) causes id drift in sibling content because showing/hiding it reflows the central layout, relocate it into a floating `egui::Window` with `.open(&mut bool)` two-way binding instead of a sibling `SidePanel`. Windows are overlaid rather than carved out of the layout, so the hero surface's row widgets keep the same available width across passes and their auto-ids stay stable without needing extra `push_id` anchors. This is the lighter alternative to the stable-slot ids above; use it only when the surface can live as a transient overlay — primary content like the torrent table itself must stay inline and rely on the row-index/slot-key convention.
- The regression suite needs **both** flavours of discarded-pass test: a toggle-driven one (`filter_toggle_across_discarded_pass_keeps_widget_ids_stable`) where the user flips state between passes, **and** a data-update one (`data_update_across_discarded_pass_keeps_widget_ids_stable`) where snapshot fields (speed, uploaded, seeders, leechers) mutate without any user action. The runtime warning class from `wrong.txt` came from the second path — a single toggle test does not exercise it.

```rust
pub(super) fn metric(ui: &mut Ui, id: impl Hash, label: &str, value: impl ToString, tone: Tone) {
    let colors = tone_colors(tone);
    let value = value.to_string();
    ui.push_id(id, |ui| {                              // outer slot id
        Frame::new()
            .fill(colors.bg)
            .stroke(Stroke::new(1.0, colors.stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if !label.is_empty() {
                        ui.push_id("metric_label", |ui| {    // inner static id
                            ui.add(Label::new(label).truncate());
                        });
                    }
                    ui.push_id("metric_value", |ui| {        // inner static id — value mutates
                        ui.add(Label::new(value).truncate());
                    });
                });
            });
    });
}

#[test]
fn filter_toggle_across_discarded_pass_keeps_widget_ids_stable() {
    let ctx = egui::Context::default();
    let output = ctx.run_ui(raw_input, |ui| {
        show(ui, &mut snapshot, &mut pending_delete, &cmd_tx, &mut table_state, tr(Language::Chinese));
        table_state.attention_only = true;
        ui.ctx().request_discard("apply attention filter");
    });
    assert!(!contains_id_warning_shape(&output.shapes));
}

#[test]
fn data_update_across_discarded_pass_keeps_widget_ids_stable() {
    // Reproduces the runtime warning class from `wrong.txt`: a tracker
    // announce returns new seeders / leechers / speed values, egui re-runs
    // the multi-pass layout, and the row widgets must keep stable ids even
    // though their text width shifted (e.g. "0 B/s" -> "120.5 KB/s").
    let ctx = egui::Context::default();
    let mut snapshot = /* ... */;
    let mut first_pass = true;
    let output = ctx.run_ui(raw_input, |ui| {
        show(ui, &mut snapshot, &mut pending_delete, &cmd_tx, &mut table_state, tr(Language::Chinese));
        if first_pass {
            first_pass = false;
            for torrent in &mut snapshot.torrents {
                torrent.current_speed_bps = torrent.current_speed_bps.saturating_add(120_000);
                torrent.uploaded_bytes = torrent.uploaded_bytes.saturating_add(2_500_000);
                torrent.last_known_seeders =
                    Some(torrent.last_known_seeders.unwrap_or(0).saturating_add(4));
                torrent.last_known_leechers =
                    Some(torrent.last_known_leechers.unwrap_or(0).saturating_add(7));
            }
            ui.ctx().request_discard("simulate snapshot update");
        }
    });
    assert!(!contains_id_warning_shape(&output.shapes));
}
```

**Why this over alternatives**:
- Turning off multi-pass (`max_passes = 1`) hides the warning but keeps the layout glitch.
- Using torrent-specific ids for row widgets makes the warning worse when sorting/filtering moves the torrent to a new rect inside the same frame.
- Pinning only the outer wrapper id and trusting auto-generated inner ids works for static helpers but breaks the moment the helper is reused for live data — the data-update test is the only thing that catches that regression class before users see red rects in the log.
- A discarded-pass regression test catches the bug class without needing a live GUI.

### Convention: localized egui controls must not hard-code label width

**Symptom**: buttons, badges, combo boxes, or inline status strips look fine in one language or with short sample data, then overflow or clip once the UI shows Chinese copy, long client filenames, or tracker names.

**Cause**: `ui.add_sized([fixed_width, ...], ...)` and narrow `desired_width(...)` values turn text length into a hidden layout contract. In `egui` the widget will still render, but the label can be elided awkwardly or visually spill against a tinted frame, especially for pill-style buttons and semantic badges.

**Fix (convention)**:
- For clickable controls, prefer `Button::min_size(...)` over `ui.add_sized([fixed_width, ...], ...)` so the control keeps a minimum footprint but can still grow when the localized string is longer.
- For text-bearing controls that can surface user or filesystem data, opt into truncation explicitly: `Button::truncate()`, `Label::truncate()`, `ComboBox::truncate()`.
- `ui.set_max_width(...)` on a label that already uses `.truncate()` is redundant and can leave dead space inside `horizontal_wrapped` — drop the cap and rely on truncation + hover_text instead.
- Keep the tint/background subtle for semantic widgets; reserve stronger colors for the text/border so truncated controls stay readable on light surfaces.
- When a value can be much longer than the label (`client`, torrent name, log line), attach hover text to the truncated control so the full value is still discoverable.

```rust
fn toolbar_action(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    button: egui::Button<'_>,
    min_width: f32,
) -> egui::Response {
    ui.push_id(id, |ui| ui.add(button.min_size(egui::vec2(min_width, 30.0))))
        .inner
}

ui.add(
    egui::Label::new(egui::RichText::new(&snapshot.active_client_filename).strong())
        .truncate(),
)
.on_hover_text(&snapshot.active_client_filename);

egui::ComboBox::from_id_salt("client_combo")
    .width(178.0)
    .truncate()
    .selected_text(&state.selected_client);
```

**Why this over alternatives**:
- Shortening every translation avoids the immediate overflow but forces copy compromises into the i18n layer.
- Letting controls wrap vertically inside dense toolbars/tables makes row heights unstable and harms scanability.
- `min_size + truncate + hover` keeps the layout predictable across locales without hiding the full value from the operator.

### Convention: desktop UI is a borderless light theme — fills, not strokes, carry hierarchy

**What**: `joal-app` follows a "white on off-white" visual system. Surfaces are separated by fill contrast, not by 1px borders. Every `Stroke` on a content frame, badge, metric, button face, or tone-tinted block is `Color32::TRANSPARENT` (or `Stroke::NONE`); the only strokes that remain are the deliberately faint `widgets.inactive.bg_stroke` on default buttons and a near-invisible `window_stroke`.

**Why**:
- The previous incarnation drew a 1px tinted border around every container (status badges, metrics, the speed chart, the log panel, table rows). The result read like a spreadsheet — visually fragmented, low contrast, no focal point. Removing the borders and letting `theme::app_background()` (`#F4F6F8`) sit behind `theme::surface()` (`#FFFFFF`) made the table-as-hero layout legible at a glance.
- Future contributors will reflexively reach for `Stroke::new(1.0, ...)` to "improve separation". This regresses the entire visual system and should be caught in review.

**Fix (convention)**:
- All `Frame` helpers in `crates/joal-app/src/ui/theme.rs` (`panel_frame`, `inset_frame`, `tone_frame`, `badge`, `metric`) MUST use `Stroke::NONE`. Do not add `.stroke(Stroke::new(...))` to any new frame helper in this crate.
- Every `Tone` in `tone_colors(...)` keeps `stroke: Color32::TRANSPARENT`. The stroke field stays in `ToneColors` so callers can still type the struct, but its value must remain transparent.
- Corner radii are quantised to three values via the module constants `CR_BADGE = 4`, `CR_INSET = 6`, `CR_PANEL = 8`. Don't introduce new radii (the pre-rewrite code mixed 5 and 8 — don't go back). Use `CR_BADGE` for pills/chips, `CR_INSET` for inset surfaces / default widgets / `metric`-style cards, `CR_PANEL` for the outer cards that frame whole sections. Divider strips and edge-to-edge background bars (e.g. `theme::strip_frame`) use `CornerRadius::ZERO` directly — these are not cards, so they don't belong on the `CR_*` scale. The constants are reserved for surfaces that visually act as containers; `ZERO` is the explicit absence of rounding, not a fourth value on the scale.
- Text uses a strict three-tier gray hierarchy from `theme.rs`:
  - `text_primary()` (`#111827`, near-black) — names, numbers, percentages, anything the operator scans for.
  - `text_secondary()` (`#6B7280`, mid-gray) — column headers, body copy, badge captions, secondary speeds.
  - `text_tertiary()` (`#9CA3AF`, light-gray) — timestamps, client filenames, log lines, "last announced" hints. Anything the operator should not focus on.
- Do not hard-code `Color32::BLACK` / `Color32::GRAY` / `Color32::from_rgb(...)` for text. Always go through the three accessors so theme tweaks stay centralised.
- `Tone` semantic roles are fixed:
  - `Success` — healthy / running / mark-as-completed.
  - `Danger` — destructive / stop / delete (use only when the action is irreversible or stops the engine).
  - `Warning` — attention required (zero-leecher torrents, recoverable issues).
  - `Info` — pending / informational.
  - `Accent` — upload/highlight + the visual ancestor of the primary button.
  - `Neutral` — default chrome; not for state.
  - Don't repurpose `Warning` for a stop button or `Danger` for "pending"; the operator's mental model depends on the colour-to-meaning mapping.

**Anti-patterns**:
- Adding `Stroke::new(1.0, theme::border())` (or any non-transparent stroke) to a new frame "for clarity". The fix is `fill` contrast, not a border.
- Introducing a fourth corner radius value because the existing three "don't quite fit". The fit is intentional; pick the closest.
- Reaching past the text accessors into `Color32::from_rgb(...)` for body copy. If you need a new shade, add it as a typed accessor in `theme.rs` and use it everywhere.

### Convention: desktop UI buttons go through `theme::primary_button` / `secondary_button` / `tone_button`

**What**: `crates/joal-app/src/ui/theme.rs` exposes a button family (`primary_button`, `secondary_button`, `tone_button`, plus `_enabled` variants for disabled-aware controls). New UI code in this crate calls one of these helpers; raw `ui.add(egui::Button::new(...))` is reserved for legacy paths still being migrated.

**Why**:
- The helpers enforce three invariants in one place: (a) the `push_id` wrapping required by `Convention: egui discarded-pass UI must pin widget ids explicitly`, (b) `Button::min_size(...)` instead of `add_sized` per `Convention: localized egui controls must not hard-code label width`, and (c) the visual hierarchy that distinguishes the primary action from secondary chrome. Bypassing the helpers re-introduces all three regression classes at once.
- The button visual hierarchy is meaningful, not cosmetic. `primary_button` is reserved for the **single most important action on its surface** (Add Torrent on the top bar, Save & Restart on the config panel). Two primary buttons on the same surface defeats the focal-point logic entirely.

**Fix (convention)**:
- Prefer `theme::primary_button(ui, id, label)` for a surface's single primary action. If a panel needs two equally-weighted primary actions, redesign the panel — don't double up the styling.
- `theme::secondary_button(ui, id, label)` is the default for everything else: cancel, toggle, navigation. It auto-picks up the borderless `widgets.inactive` style.
- `theme::tone_button(ui, id, label, tone)` is the right choice when the action carries semantic colour: `Tone::Danger` for delete confirmation, `Tone::Success` for "start engine", `Tone::Warning` for "mark as zero-leecher", etc. Use this instead of writing a one-off `Button::fill(...)` block.
- For controls that need to be disabled when invalid, use the `_enabled(ui, id, label, enabled)` variant. Do not wrap a normal helper in `ui.add_enabled_ui(...)` — the helpers already handle the disabled visuals and keep the `push_id` path consistent.
- When introducing a new button helper, mirror the existing pattern: `push_id` outer wrapper → `Button::min_size(egui::vec2(min_width, 30.0))` → optional `.truncate()` for translated labels → return the inner `Response`. Do NOT use `ui.add_sized(...)`.

**Anti-patterns**:
- Calling `ui.add(egui::Button::new("..."))` in new code under `crates/joal-app/src/ui/`. Even if the label looks short and English, it will end up translated and will lose `push_id` protection.
- Painting a "fake primary" with `Button::fill(theme::accent())` instead of using `primary_button`. The helper exists precisely to keep the primary-button look in one place; ad-hoc copies drift.
- Putting more than one `primary_button` on the same panel/dialog. The visual contract is "one primary action per surface".

### Convention: hero surface owns `CentralPanel`; auxiliaries are resizable `TopBottomPanel`s in strict order

**What**: The desktop layout in `crates/joal-app/src/ui/mod.rs` follows a fixed Panel order so the torrent table — the app's hero surface — automatically claims all remaining vertical space:

```rust
// 1. Top: status + action buttons + table toolbar — content-sized, no resizable
egui::TopBottomPanel::top("top_panel").show(ctx, |ui| { /* ... */ });

// 2. Bottom telemetry (speed chart + log): resizable with explicit bounds
egui::TopBottomPanel::bottom("telemetry_panel")
    .resizable(true)
    .default_height(150.0)
    .height_range(110.0..=320.0)
    .show(ctx, |ui| { /* ... */ });

// 3. (optional) Footer status strip: exact height, NOT resizable
egui::TopBottomPanel::bottom("footer_status").exact_height(24.0).show(ctx, |ui| { /* ... */ });

// 4. Central: the torrent table (hero) — claims everything left
egui::CentralPanel::default().show(ctx, |ui| { /* torrent_table::show(...) */ });
```

**Why**:
- egui assigns space to panels in the order they are registered, and `CentralPanel` takes whatever is left. Declaring `CentralPanel` first or burying the table inside a `BottomPanel` collapses the table to whatever min-size the inner content reports — exactly the symptom the layout rewrite was undoing (PRD: "main UI layout exposes a larger primary table area").
- The PRD also requires "adjustable panes rather than fixed percentages". `resizable(true) + default_height + height_range` is the contract that satisfies it: the user gets a sensible default and can drag the divider, but the bounds prevent them from accidentally hiding the telemetry or starving the table.
- Footer-style strips (status bar, version line) must use `exact_height(...)` and stay non-resizable, otherwise the user can drag them into nothingness and the surface becomes unrecoverable.

**Fix (convention)**:
- The torrent table — and any future "hero" surface in this app — lives in `CentralPanel`. Do not put the table inside a `BottomPanel` "because it fits there for now".
- Every auxiliary `TopBottomPanel::bottom(...)` that hosts non-trivial content (chart, log, config, side panel) MUST set `resizable(true)`, `default_height(...)`, and `height_range(min..=max)`. Pick a `min` that keeps the panel's primary widget readable at its smallest and a `max` that still leaves the table ~50% of the window.
- Fixed-height strips (footer status, decorative dividers) use `exact_height(...)` and stay non-resizable.
- Panel registration order is Top → Bottom (telemetry) → Bottom (footer) → Central. If you add a new auxiliary panel, slot it before `CentralPanel`, never after.

**Anti-patterns**:
- `egui::CentralPanel::default().show(ctx, |ui| { ui.vertical(|ui| { /* status, table, chart, log */ }) })` — collapses the layout back to a single scroll surface and loses the resize affordance the PRD demands.
- A `BottomPanel` with `default_height(400.0)` and no `height_range` cap — lets the auxiliary panels swallow the table.
- A `resizable(true)` footer status strip — the user can drag it to zero height and lose the engine indicator.
