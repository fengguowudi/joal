# Backend Development Guidelines

> JOAL — Java 11, Spring Boot 2.7.3, Maven, Log4j2, Lombok. A BitTorrent seeder that exposes a STOMP WebSocket UI but no REST API, and persists state to JSON + the filesystem (no database).

---

## Guidelines Index

| Guide | Description |
|-------|-------------|
| [Directory Structure](./directory-structure.md) | `core/` vs `web/` boundary, feature-based packaging, event/payload mirror pattern |
| [Persistence (No Database)](./database-guidelines.md) | `config.json` + `.torrent` filesystem layout, `JoalConfigProvider`, `TorrentFileProvider` archive rules |
| [Error Handling](./error-handling.md) | Checked vs unchecked taxonomy, the 3 canonical catch shapes, exception skeletons with `serialVersionUID` |
| [Logging Guidelines](./logging-guidelines.md) | `@Slf4j` + Log4j2 via SLF4J, per-level usage matrix, parameterised `{}` format |
| [Quality Guidelines](./quality-guidelines.md) | `@Inject` constructor DI, Lombok subset, `final` everywhere, JUnit 5 + Mockito + AssertJ testing |

---

## Read order when starting a new task

1. Skim **Directory Structure** to find the package your change belongs in.
2. Read **Quality Guidelines** for the Lombok / DI / testing expectations.
3. Read **Error Handling** and **Logging Guidelines** before writing any `try`/`catch` or `log.*`.
4. **Persistence** only matters when you touch config or torrent files.

---

**Language**: All spec documentation is written in **English**.
