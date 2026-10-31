# Arquitectura de o3-server

Objetivo: reemplazo en Rust de T3 headless, intercambiable para los clientes (lib-t3-rust, O3 desktop, O3 Android).
RSS objetivo: < 100 MB, ideal < 50 MB, medido con clientes conectados y proveedores activos (el proceso del servidor;
los CLI de proveedores son procesos aparte).

Fuentes normativas: `docs/research/t3-wire-contract.md`, `docs/research/client-usage.md`,
`docs/research/omabridge-embedding.md`. Estado: `docs/COMPATIBILITY.md`, generado desde `docs/compat-status.json`.

## Decisiones

1. **Compatibilidad = conformidad.** `tests/client_compat.rs` usa la lib real y debe poder correr también contra un T3 real
   (`O3_CONFORMANCE_BASE`). Una RPC solo es ✅ con un test así. Se anuncia solo lo implementado: capacidades ausentes = apagadas.
2. **Perfil compatible.** Se respeta `T3CODE_HOME` (por defecto `~/.t3`), con `userdata/environment-id`, `secrets/*.bin`,
   `settings.json`, `server-runtime.json`. Comparte la clave de firma, así que los tokens existentes siguen valiendo.
   Rehúsa arrancar si `server-runtime.json` apunta a un PID vivo.
3. **Jamás se escribe la base de T3.** Estado propio en `userdata/o3.sqlite`. El esquema de T3 cambia cada nightly; la
   importación de hilos desde `statev2.sqlite` se hace sobre una copia consistente, solo lectura.
4. **Binario único multi-nombre.** Responde la CLI que usa el desktop (`auth session issue`, `__ssh-helper`, `service install`,
   `serve`), de modo que un symlink `t3` lo haga drop-in sin tocar el desktop.
5. **Memoria.** Tokio `multi_thread` con `worker_threads(2)`, `max_blocking_threads(2)` y stacks de 512 KiB; estado `Arc<State>` con SQLite bajo `Mutex`, mimalloc, proyecciones en SQLite y no en RAM, caché SQLite acotada,
   sin mmap, transporte genérico `serve_io` para TCP y `DuplexStream` de OmaBridge (§4.3), snapshots serializados en streaming y buffers por suscriptor acotados.
6. **Proveedores por subproceso.** Claude: CLI `stream-json` directo, nada de Node. Codex: `app-server` JSON-RPC por stdio.
7. **OmaBridge dentro del proceso**, ruteando streams al handler HTTP/WS sin TCP loopback, detrás de un feature.
8. **Sin web SPA, MCP OAuth, guest share ni T3 Connect** en el alcance inicial.
