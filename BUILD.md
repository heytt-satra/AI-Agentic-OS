# Building & distributing Jarvis (zero-install, any OS)

## The distribution model

A native program can't be one file for all operating systems, so we ship **one
self-contained binary per OS**. Each binary needs **nothing installed alongside
it** — the user downloads their OS's file and runs it.

This works because every dependency is either pure Rust or compiled *into* the
binary:

| Concern | How it's self-contained |
|---|---|
| Database | `rusqlite` **bundled** — SQLite C source compiled into the binary |
| TLS / HTTPS | `reqwest` + **rustls** — pure-Rust TLS, no OpenSSL |
| Async/HTTP/JSON | tokio, reqwest, serde — pure Rust |
| Embeddings (planned) | `candle` — pure-Rust ML, model weights downloaded to a cache dir at first run (data, not an install) |

**The rule that keeps this true:** never add a dependency that needs a *native
runtime library* installed on the user's machine. Specifically, do NOT use
`onnxruntime`/`fastembed` for embeddings — use `candle`. (See ARCHITECTURE.md.)

## Build locally

```bash
cargo build --release          # -> target/release/jarvis(.exe)
```

The only BUILD-time requirement is a C compiler (for SQLite) + Rust — both are
standard. At RUN time the binary is self-contained.

## Release for all platforms

Push a version tag; GitHub Actions builds Windows/macOS/Linux binaries and
attaches them to a Release (see `.github/workflows/release.yml`):

```bash
git tag v0.1.0
git push origin v0.1.0
```

## Code signing (Windows SmartScreen)

Unsigned Windows binaries trip the SmartScreen "unknown publisher" wall, which
scares off strangers. The release workflow signs the Windows binary
automatically **once a cert is configured** — until then it ships unsigned and
prints a note, so releases keep working.

To enable signing, add two repo secrets (Settings -> Secrets and variables ->
Actions):

- `WINDOWS_CERT_BASE64` — your code-signing certificate (`.pfx`) as base64.
  Create it with: `base64 -w0 cert.pfx` (or `certutil -encode` on Windows).
- `WINDOWS_CERT_PASSWORD` — the .pfx password.

The workflow decodes the cert to a temp file, signs with `signtool` (SHA256 +
RFC-3161 timestamp so signatures outlive the cert), deletes the cert, and
verifies. Certs: a standard OV/EV code-signing cert (~$100-400/yr) or the
cheaper **Azure Trusted Signing**. macOS/Linux binaries are unaffected.

## Runtime data (not install)

Jarvis writes a few files next to where it runs: `.env` (your keys),
`jarvis.db` (memory), `workspace/` (file-tool sandbox), and a model cache for
embeddings. None of these require installation — they're created on first use.

## Alternatives considered

- **Single universal artifact (WASM / container):** would run one file
  everywhere, but a personal agent needs native filesystem/shell/mic access,
  which WASM sandboxes and containers complicate. Native per-OS binaries are the
  right call for this product.
