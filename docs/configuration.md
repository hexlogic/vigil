# Configuration

Vigil reads configuration from, lowest to highest precedence:

1. Built-in defaults.
2. A TOML file: `--config <path>` or `VIGIL_CONFIG`, otherwise `./vigil.toml` if it exists.
3. `VIGIL_*` environment variables (nested keys are separated by `__`).

See [`vigil.example.toml`](../vigil.example.toml) for a complete example.

| Key | Env var | Default | Description |
|---|---|---|---|
| `listen` | `VIGIL_LISTEN` | `0.0.0.0:5800` | Address the HTTP server binds to. |
| `data_dir` | `VIGIL_DATA_DIR` | `data` | Directory for the SQLite database (`vigil.db`). Created if missing. |
| `log_format` | `VIGIL_LOG_FORMAT` | `text` | `text` or `json`. |

Log verbosity is controlled by `RUST_LOG` (default `info`).

## Commands

- `vigil serve` — start the HTTP server. `GET /healthz` returns `200 ok` when the database is reachable.
