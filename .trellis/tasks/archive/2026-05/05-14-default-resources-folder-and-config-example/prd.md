# Default resources folder & config example

## Requirements

1. Make `--joal-conf` CLI argument optional. When omitted, default to `<exe_dir>/resources/`.
2. Create `resources/config.example.json` as a documented example config.

## Acceptance criteria

- Double-clicking `joal-desktop.exe` (no args) loads `resources/` next to the exe.
- `--joal-conf` still works as an override.
- `config.example.json` exists with all fields documented.
