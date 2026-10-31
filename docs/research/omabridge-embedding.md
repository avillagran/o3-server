# Embedding OmaBridge inside o3-server

Research note. Scope: how to run OmaBridge in the `o3-server` process instead of a separate `omabridge serve` service, what that needs, and what it costs.

Sources read (all local, read-only):
- `/home/node/Work/omabridge` at `cd0de3a` ("Release 0.2.4"). Crate `omabridge` 0.2.4, edition 2024, MSRV 1.91, iroh `=1.4.1` (`tls-ring`), hyper 1, tokio 1.
- `~/Obsidian/Dev/Projects/omabridge/MEMORY.md` (grepped, not read whole).
- Clients: `/home/node/Work/omarchy-t3-code/src/omabridge_*.rs` (o3 desktop, "D") and `/home/node/Work/t3code-nightly/apps/server/src/environment/OmaBridgeEndpoints.ts`.
- Sibling notes in this directory: `client-usage.md`, `t3-wire-contract.md`.

Line numbers refer to omabridge `cd0de3a` and will drift.

Measurements were made on this machine (Linux 7.2, 24 cores, rustc 1.99.1) with the release binary built in `/tmp/omab-target` and an isolated `HOME` under `/tmp/omab-run`. Nothing under `~/.config` or the installed services was touched. Two OmaBridge processes of the real user were running throughout (PIDs 999 and 3995, `~/.config/omarchy/plugins/...`) and were left alone.

---

## 1. Library API to embed

### 1.1 What `omabridge serve` does

`Command::Serve` in `src/main.rs:841`, in order:

1. Parse `--service name=127.0.0.1:port` (repeatable) into `BTreeMap<String, SocketAddr>`. Only loopback targets with a non-zero port are accepted (`network.rs:1507`).
2. Optionally load an enrollment authority from `--enrollment-config`: `delegated_enrollment::load_authority(store, &path) -> Arc<dyn EnrollmentAuthority>`.
3. `Node::start_reserved_with_configuration(store, targets, options, store.reserve_runtime()?, authority, t3_runtime)`.
4. `store.publish_endpoint(&node.addr(), &service_names)`, which writes `endpoint.json`. This is what `Store::active_endpoint()` reads. The CLI, `Manager` and the invite flow all gate on it.
5. Print the `{"event":"listening",...}` line, wait for SIGTERM/SIGINT, `node.shutdown().await`.

### 1.2 Entry points

| Piece | Where | Notes |
|---|---|---|
| `Store::open(path, name)` | `lib.rs:802` | Identity (iroh secret key) and peers, as `state.json` under the state dir (default `~/.local/share/omabridge`, `lib.rs:52`). Requires a private 0700 non-symlink dir. All operations are synchronous `std::fs` plus `fs2` flock. |
| `Store::reserve_runtime() -> RuntimeReservation` | `lib.rs:431` | Exclusive `runtime.lock`. **A second `serve` on the same state dir fails**, so o3-server cannot coexist with the installed `omabridge-serve.service` on the same dir. |
| `NetOptions { bind, relay, compress_t3 }` | `network.rs:108` | `Relay::{Public, Custom(url), Disabled}`; the choice persists in `relay.json`. `NetOptions::loopback()` = no relay, `127.0.0.1:0`. |
| `Node::start*` | `network.rs:1411-1490` | The embedding point. The most general form is `start_with_t3_issuer(store, services, options, reservation, authority, t3_runtime, t3_issuer)`. Returns `Node` with `addr()` and `shutdown()`. `Drop` aborts the tasks. |
| `Client` | `network.rs:1989` | Outbound: `pair`, `open`, `forward`, `catalog`, `manage`, `drafts`, `enroll`, `t3_pair`, `announce`. Used by CLI commands and the mobile SDK. Not needed on a pure host. |
| `connect_through_server`, `forward_through_server`, `t3_pair_through_server`, `find_peer`, `forward_target` | `network.rs` | CLI-side helpers that talk to a running server over the control socket. |
| `manage::Manager` | `manage.rs:126` | JSON action engine behind `omabridge manage`; also the remote-admin engine. `Manager::from_env()` bakes in systemd (see 1.4). |
| `drafts::execute(&Store, Option<peer>, &Value) -> Value` | `drafts.rs:233` | Shared-drafts store; pure file I/O. |
| `catalog::read_for_peer`, `catalog_publisher::Publisher` | | Client catalog read and publish. |
| `relay::RelayServer`, `hosting::host_loop` | | Private iroh relay host. |
| `gateway::gateway_loop(store, endpoint, options)` | `gateway.rs:406` | Public HTTPS gateways (TLS listener -> peer service via iroh). |
| `mcp::server::start(GatewayStore, ServerConfig)` | `mcp/server.rs:1131` | OAuth MCP gateway, a separate process today (`omabridge mcp serve`). |

### 1.3 What `Node::start_with_t3_issuer` spawns

It binds the iroh `Endpoint` and then spawns five tasks unconditionally (`network.rs:1520-1545`):

1. **accept loop**: 64-permit semaphore, per-peer cap, an "unknown device" slot pool, 8 s setup timeout. Each connection runs `serve_connection` (`network.rs:1698`).
2. **`sync_loop`** (`:481`): announces the address to every peer every 300 s, on change, and on a 10..160 s retry backoff. It polls every 2 s (`NAME_CHECK`). It calls `manage::rollback_due(.., &Systemctl)`, which can restart the systemd unit.
3. **`control_loop`** on `control.sock` (`:528-590`): the unix-socket server that CLI `forward`/`connect`/`t3-pair`/`manage`/announce use. Chmod 0600.
4. **`hosting::host_loop`** (`hosting.rs:354`): polls `relay-host.json` every 5 s and starts or stops an `iroh-relay` server.
5. **`gateway::gateway_loop`** (`gateway.rs:406`): polls `gateways.json` every 5 s and starts or stops TLS listeners.

There are no feature switches for 2-5.

### 1.4 What `serve_connection` handles

The first bi-stream carries a length-prefixed JSON `Request`:

- `Pair`, `Hello`, `Catalog`, `Enroll`, `DelegatedEnroll`: control only. Store and catalog operations; the last two call the `EnrollmentAuthority` trait.
- `T3Pair`, `T3HostPair`, `T3DelegatedPair`: issue a one-use T3 pairing credential for a peer holding a `t3` grant.
- `Drafts`: `drafts::execute`.
- `Manage`: `manage::serve_remote`. Requires the per-peer `admin` bit.
- `Open { service, public, compression }`: the data path. It checks the grant, calls `ServiceTargets::resolve`, **`TcpStream::connect(target)`** (`network.rs:1948`), answers `Ready`, then runs `Pipe::transfer(socket)` until either side closes. Every 1 s it re-checks that the grant is still valid, so revocation cuts open streams within about 1 s.

### 1.5 What assumes a T3 subprocess, a runtime file, a loopback port or the CLI

These need refactoring for in-process use.

| Assumption | Where | In-process replacement |
|---|---|---|
| **Data path is loopback TCP**: `TcpStream::connect(SocketAddr)`, and `services` must be loopback with port != 0 | `network.rs:1507`, `:1948`, `ServiceTargets.fixed` | `enum Target { Tcp(SocketAddr), Local(Arc<dyn LocalService>) }`. For `Local`, create `tokio::io::duplex(N)`, pass one half to `Pipe::transfer` (already generic over `AsyncRead + AsyncWrite + Unpin`, `network.rs:2020`) and hand the other to the server's connection handler. See 4.3. |
| **T3 port from `server-runtime.json`** (`--t3-runtime`) | `ServiceTargets::resolve`, `network.rs:~1665` | Not needed when the target is in-process. |
| **Pairing credential via CLI**: `T3Issuer` runs `<t3> auth pairing create --base-dir ... --ttl 2m --label ... --json`, after `t3_protocol::verify_environment(http://target/)` over loopback HTTP, and reads `<base_dir>/userdata/environment-id` | `t3_pair.rs:48-170` (`T3Issuer` is a concrete struct, not a trait) | Make `T3Issuer` a trait (`issue(label) -> T3Pairing`, `environment_id()`), implemented by o3-server's auth module. Skip `verify_environment` and the CLI spawn. The label rules (`omabridge:<peer>`, `omabridge-delegated:<bridge>:<client>`) must stay so revocation by label keeps working. |
| **Delegated enrollment via CLI**: `LocalT3Authority` runs `<t3> auth session issue --scope orchestration:read ...` after a loopback `verify_environment` | `enrollment.rs:300-360` | `EnrollmentAuthority` is already a trait (`enrollment.rs:60`). Implement it in o3-server. No omabridge change. |
| **`T3Issuer::detect()`** searches `$HOME/.t3`, `runtime/service-state.json`, PATH and `~/.local/bin/t3` | `t3_pair.rs:~75`, called in `network.rs:~1550` when `t3` is published | Pass an explicit issuer (`Some(Some(x))`). Never auto-detect in embedded mode. |
| **Publishing is a systemd unit.** `publish`/`unpublish`/`snapshot.published`/`serves_itself`/`omabridge_already_running` all read `omabridge-serve.service` in `~/.config/systemd/user` and call `systemctl --user` | `manage.rs:629,1028-1125`, `units.rs` | A `ServeBackend` trait in `Manager` with `published()`, `publish(services)`, `unpublish()`, `restart()`. The embedded backend writes a `services.json` and the host reloads in-process. `units::Runner` is already injectable but `unit_services(path)` reads the unit file directly. |
| **`Deferred` / `rollback_due`**: remote restart/stop queue `systemctl --no-block restart omabridge-serve.service` | `manage.rs:1702-1750`, `:1903`, `network.rs:489` | The same backend trait. The embedded `restart` is a graceful in-process restart of the Node. |
| **`Manager::from_env`**: `t3_runtime = ~/.t3/userdata/server-runtime.json`, `ssh_target 127.0.0.1:22`, `mcp_target 127.0.0.1:8787`, and `binary = current_exe()` is written into unit files | `manage.rs:316-350` | An embedded constructor with explicit values. `binary` becomes unnecessary. |
| **`catalog_publisher::poll_once`** spawns the desktop executable (`connections-view`) | `catalog_publisher.rs:423` | Not needed. `publish_auto` (no desktop view, no spawn) is the path for o3-server. |
| **`headless.rs`** spawns an owned `t3` binary | `main.rs` `Command::T3`, `headless.rs` | Dead code for o3-server. |
| **`reqwest`-based probes in `manage`** (public-address check) | `manage.rs:1536` | Compile out or keep. |
| **`t3_protocol` blocking HTTP** (`verify_environment`, `pair_bearer`) | `t3_pair.rs:140`, `enrollment.rs:111-216` | Needed only by the CLI-based issuer and enrollment, i.e. disappears with the trait refactor. `bridge_catalog::{publish,reconcile}` still needs it (`catalog.rs`, `catalog_client.rs`, `catalog_publisher.rs`). |
| **Process-global state**: `HOME`, `XDG_*`, `OMABRIDGE_CERT_DIR` env reads, `/etc/letsencrypt/live`; `process::exit` only in `main.rs` (not in the lib) | `lib.rs:52`, `hosting.rs:28` | Pass explicit paths. |
| **Sync blocking I/O inside async paths**: `Store` is flock plus `std::fs`; `store.peers()` is called from every `Open` stream's 1 s `allowed()` recheck (`network.rs:~1965`) and for each incoming connection | `lib.rs`, `network.rs` | Fine at personal scale (one 1 MiB-capped JSON read per second per live stream). Wrap hot calls in `spawn_blocking` or cache `Store` snapshots with an mtime check. |
| **rustls crypto provider**: omabridge uses `ring` only (no aws-lc in the tree) | `Cargo.toml` | o3-server must use ring too, or install the default provider explicitly, to avoid pulling in two providers. |

### 1.6 What shells out to a CLI

Bridge side:
- `t3 auth pairing create` (`t3_pair.rs:146`)
- `t3 auth session issue` (`enrollment.rs:322`)
- the desktop `connections-view` (`catalog_publisher.rs:430`)
- `systemctl --user` (`units.rs:225`)
- `t3` headless launch (`headless.rs:78`)

Client side (o3 desktop), the calls that must keep working:

| Call | File | What it needs from the bridge |
|---|---|---|
| `omabridge --state-dir S --name ot3 peers` | `omabridge_route.rs:126`, `omabridge_share.rs:117` | Read-only; works without a server. The state dir must be shared. |
| `omabridge manage` (JSON on stdin) | `omabridge_device.rs:36` | CLI -> `Manager` -> control.sock when a server runs. Actions: snapshot, `invite`, `approve`, `revoke`, `pair`, `publish`, `unpublish`, `relay-set`, `gateway-set`, ... |
| `omabridge forward --peer P --service t3 --listen 127.0.0.1:0` | `omabridge_route.rs:162`, `omabridge_share.rs:80` (systemd unit) | `forward_through_server` over control.sock, so the device keeps one iroh identity. |
| `omabridge connect` (stdio), `t3-pair --peer P` | `omabridge_share.rs:125` | Over control.sock; `t3-pair` returns `{environmentId, credential, label}`. |
| A shell wait on `control.sock` before starting a forward | `omabridge_share.rs:80`, `omabridge_route.rs:151` | The embedded host must create `<state_dir>/control.sock` with the same protocol. |

Consequence: if o3-server keeps the control socket (`control_loop`) and the same state dir, **the installed `omabridge` CLI and the desktop keep working unchanged**.

The server side of the clients (`OmaBridgeEndpoints.ts`, in the t3code-nightly working copy) spawns `omabridge manage` with `{"action":"endpoints"}` and a 1 s timeout. o3-server should instead call the same code in-process (`store.service_routes(now)`, `manage.rs:399`), which removes the subprocess.

---

## 2. Subsystems clients need, and what can be gated off

### 2.1 How clients detect and use the bridge

- **Server -> client announcement**: `ServerEnvironmentConfig.directEndpoints` (`client-usage.md`, `t3-wire-contract.md:15132`). Upstream's `ServerDirectEndpointKind` is `["lan","tailnet"]` (`t3-wire-contract.md:15321`). The nightly working copy adds `kind: "omabridge"`; older clients ignore it via `ForwardCompatibleArray`. Each entry is `{kind:"omabridge", httpBaseUrl:"https://<name>:<port>/"}`.
- **Candidate source**: gateway routes (`routes::ServiceRoute`, `routes.rs`) announced by paired gateway devices through `Hello`. Only fresh ones (TTL 600 s) whose owner still has consent are exported. The server then GETs `<url>/.well-known/t3/environment` with no credentials and keeps the route only if `environmentId` matches (`OmaBridgeEndpoints.ts`). o3-server must serve that descriptor (required anyway, `client-usage.md:27`).
- **Catalog**: phone -> `Request::Catalog` over iroh. The answer is per-peer from `client-catalog.json`, filtered by the caller's authenticated id and `t3` grant. It is metadata only (`docs/client-catalog.md`).
- **Native pairing**: `T3Pair`/`T3HostPair`/`T3DelegatedPair` return a one-use credential. The device then exchanges it over the `t3` data path (`POST /oauth/token`).
- **Desktop**: reads peers via the CLI, starts `forward` per environment (route `auto`/`prefer`/`only`), tracks the active peer, and installs local forward units.

### 2.2 Subsystem matrix

| Subsystem | Needed by default? | Notes | Gate |
|---|---|---|---|
| **Iroh endpoint, `Open` data path (`t3`)** | **Yes**, core | Phone and desktop reach the T3 HTTP/WS over QUIC. | always |
| **Pairing / invitations / peers / grants / revoke** (`Store`, `Pair`, `Hello`, `refresh_peer`) | **Yes** | Without it nothing is authorized. | always |
| **Control socket** (`control_loop`) | **Yes** for the desktop and installed CLI | `forward`, `connect`, `t3-pair`, `manage` rely on it so they avoid a second endpoint with the same identity. | `control-socket` (default on) |
| **T3 pairing issuance** (`T3Pair*`) | **Yes** for "pair my phone via the bridge" | Needs the `T3Issuer` trait in-process. | `t3-pair` |
| **Device catalog** (`Catalog`, `catalog::read_for_peer`) | **Yes** for the mobile multi-host list | Pure file reads. `catalog_publisher::publish_auto` rewrites it. | `catalog` |
| **Mobile admin** (`Manage` + `serve_remote`, features `mobile-admin-v1`) | Optional but used by Android | Needs the `ServeBackend` abstraction. A read-only subset (`read`, `endpoints`, `invite`, `approve`, `revoke`, `rename`) works without any publish backend. | `remote-manage` |
| **Shared drafts** (`Drafts`, `shared-drafts-v1`) | Optional, enabled by default for desktop/Android composers | Pure store; no network beyond QUIC. Quota 16 MiB, 1024 live docs. | `drafts` |
| **Delegated / all-hosts enrollment** (`Enroll`, `DelegatedEnroll`) | Only for multi-host bridge setups | Trait-based, denied by default. | `delegated-enrollment` |
| **Public gateway** (`gateway_loop`, TLS listeners, `hosting` cert handling, `x509-parser`, `rustls-pemfile`) | Only on a host with a public address + wildcard cert | The path from "phone on 4G, unmodified Android app" to a normal HTTPS URL. | `gateway` |
| **Private relay host** (`hosting::host_loop`, `relay::RelayServer`, `iroh-relay` `server` feature) | Rarely | Heaviest dependency subtree (ACME, rcgen, websockets, dashmap, ...). | `relay-host` |
| **MCP federation / OAuth gateway** (`mcp/*`, hyper server, reqwest) | No. Separate process today (`omabridge mcp serve`), reached through the `mcp` service target. | Keep as a separate optional binary or plug-in; embed only if o3-server wants to host it. | `mcp` |
| **Egress** (`egress.rs`), **GUI** (`gui`/eframe), **units/systemd** (`units.rs`, `Systemctl`), **headless T3 launcher** | No | Not relevant to the server. | `egress`, `gui`, `systemd` |

The 14 `manage::FEATURES` strings (`control-socket`, `forward-through-server`, `connect-stdio`, `remote-manage`, `relay-host`, `gateways`, `delegated-enrollment-v1`, `t3-pair-v1`, `mcp-service`, `service-routes`, `gateway-domain`, `all-hosts-invite-v1`, `mobile-admin-v1`, `shared-drafts-v1`) are what clients read from `capabilities`/`read` to decide which UI to show. **A gated-off subsystem must be dropped from that list**; otherwise clients will offer it.

---

## 3. Measured footprint

### 3.1 Method

- Binary: `cargo build --release --locked --bin omabridge` with `CARGO_TARGET_DIR=/tmp/omab-target`. Build time 2m53s on 24 cores, 807 crates in the normal dependency tree. Stripped size is **23.7 MB**. The profile is thin LTO, codegen-units=1, strip.
- Setup: isolated `HOME=/tmp/omab-run/h`, state dirs `a` (host) and `b` (client) with fresh identities, `--relay none --bind 127.0.0.1:..`. A python `http.server` on `127.0.0.1:38773` stands in for T3 (`serve --service t3=127.0.0.1:38773`). `b` paired (invite -> pair -> approve) and ran `forward` to `127.0.0.1:38774`. All traffic was loopback.
- Caveat on the numbers: `/tmp` here is **tmpfs**, so the binary's mapped text pages show up as `RssShmem` (about 11 MB) instead of `RssFile`. `RssAnon` is the real heap and stack cost. On a disk filesystem the file-backed part is shared, reclaimable page cache. Report both: total RSS as the safe upper bound, and `RssAnon` as the floor.
- The tuning runs reuse the paired `b` forward. They were not exact A/B tests of every knob (single run each).

### 3.2 Results (`omabridge serve`, `t3` published, relay off)

| State | VmRSS / VmHWM | RssAnon | Threads |
|---|---|---|---|
| Idle, 3 s after start | **16.6 MB** | 2.8 MB | 26 |
| After a paired peer's first request (one connection) | 19.6 MB | 3.3 MB | 25 |
| 8 idle open streams held | 19.6 MB | 3.3 MB | 25 |
| After a 200 MB download through the bridge | 23.9 MB | 7.3 MB | 25 |
| After 32 concurrent requests following that | 26.9 MB (HWM) | 10.3 MB | 25 |
| Client side `forward` (`b`), same session | HWM 24.7 MB | 8.5 MB | 25 |

Throughput on loopback was about 570-760 MB/s, so QUIC encryption is not the bottleneck. CPU: 0 ticks over 20 s idle and about 20 voluntary context switches per 10 s (the 2 s/5 s polling loops).

### 3.3 Baseline: bare iroh `Endpoint`

A 40-line crate (`/tmp/omab-baseline`, `presets::Minimal`, relay disabled, no address lookup, same lock file) built to `/tmp/omab-target/release/baseline` (12.0 MB stripped):

| State | VmRSS | RssAnon | Threads |
|---|---|---|---|
| Before creating the endpoint (tokio multi-thread runtime up) | 4.9 MB | 0.9 MB | 25 |
| One `Endpoint` idle | **11.1 MB** | 2.4 MB | 26 |
| Two endpoints plus one live connection | 13.5 MB | 2.8 MB | 26 |
| Same, `TOKIO_WORKER_THREADS=2 MALLOC_ARENA_MAX=1` | 13.1 MB | 2.4 MB | 4 |

So the bridge on top of a bare endpoint adds about **0.4 MB of heap and about 5 MB of mapped binary text**. The iroh `Endpoint` itself costs about 6 MB RSS (about 1.5 MB heap) once the code pages are touched. The rest is the Rust runtime and TLS. This footprint is dominated by iroh, not by OmaBridge's own code.

### 3.4 Tuning knobs (each a single run; `serve`, 32 concurrent small requests plus a 100 MB download)

| Setting | Idle RSS | Idle anon | Threads | Peak HWM | Peak anon |
|---|---|---|---|---|---|
| default (24 workers) | 16.8 MB | 2.8 MB | 26 | 25.7 MB | 9.6 MB |
| `TOKIO_WORKER_THREADS=1` | 16.7 MB | 2.4 MB | 3 | 23.9 MB | 7.7 MB |
| `TOKIO_WORKER_THREADS=2` | 16.2 MB | 2.4 MB | 4 | 23.4 MB | 7.6 MB |
| `TOKIO_WORKER_THREADS=2 MALLOC_ARENA_MAX=1` | 16.4 MB | 2.4 MB | 4 | **21.8 MB** | **5.7 MB** |
| `TOKIO_WORKER_THREADS=4 MALLOC_ARENA_MAX=1` | 16.4 MB | 2.4 MB | 6 | 21.8 MB | 5.6 MB |

Findings:
- Worker count does little for idle RSS (it changes the thread count: 26 -> 4), but it removes per-thread stacks and arenas under load. One worker still sustained about 760 MB/s.
- `MALLOC_ARENA_MAX=1` is the biggest single gain on peak heap (about -2 MB under load) and is free. In a shared server process, set one arena for the whole process (or use mimalloc/jemalloc with a small arena count) rather than per component.
- The bulk of the 16 MB at idle is binary text plus shared libs; it is not removable by runtime knobs. Shrinking it means fewer code paths linked (3.5).
- Relay mode was **not measured**: `Relay::Public` would contact n0's servers, and this task is loopback-only. Expect a small increase for the extra relay websocket and TLS state, in the low single-digit MB; verify later against the production relay config. `Relay::Disabled` or a `Custom` private relay avoid public discovery traffic. `endpoint()` already does `clear_address_lookup()` and `clear_ip_transports()` (`network.rs:1361`), so there is no DNS/pkarr discovery to turn off. Only the relay is relevant.

Conclusion: **the bridge by itself fits the budget** (21-27 MB under load, 3-10 MB heap). The whole o3-server should still be under 50 MB if the other parts (HTTP, SQLite, provider supervision) stay lean; the bridge's total footprint is about 10-15 MB RSS shared with the server's own tokio runtime and TLS stack (not additive: the runtime, hyper and rustls are shared).

### 3.5 Binary and dependency weight

The 23.7 MB binary (vs 12.0 MB for iroh alone) comes from code o3-server does not need on a normal host. From `cargo tree` (not measured per crate, because that needs a refactor):
- `iroh-relay` with `server` feature (ACME, `rcgen`, `tokio-websockets`, `dashmap`, `toml`, `tracing-subscriber`, `clap`) is linked for `hosting`/`relay` only.
- `t3-protocol` pulls `reqwest 0.12` (blocking), `tungstenite`, `p256`, `chrono`, `pulldown-cmark`, `uuid`. omabridge also depends on `reqwest 0.12` directly (`blocking`) and on `reqwest 0.13` (via iroh). Two reqwest majors are in the tree.
- `eframe` is already feature-gated (`gui`); `clap`, `qrcode` are CLI-only.
- `mcp` uses its own hyper server and `reqwest`.

---

## 4. Recommended design

### 4.1 Dependency form

Depend on omabridge as a **git dependency pinned to a rev** (not a path), e.g.

```toml
omabridge = { git = "https://github.com/avillagran/omabridge", rev = "<sha>", default-features = false, features = ["embed"] }
```

- The repo already pins `t3-protocol` the same way (`rev = 8c80c4dc...`), so the pattern is established. Use a path dependency only for local co-development, via a `[patch]`.
- Version-lock iroh to `=1.4.1`, `iroh-relay` `=1.4.1`, `reqwest =0.13.5`, `flate2 =1.1.10` (these are exact pins in omabridge). Let o3-server inherit them through omabridge's `Cargo.lock`, or copy them exactly; otherwise `cargo` will resolve a second iroh.
- Commit `Cargo.lock` and build with `--locked`. This was verified: omabridge builds `--locked` today.
- The crate currently has `[lib]` and `[[bin]]` in the same package with `default = []`; the CLI modules (`clap`, `qrcode`, `headless`, GUI) are not behind a `cli` feature. The lib already `pub mod`s nearly everything, so embedding works as-is; the features below are a **clean-up proposal** for omabridge, to be upstreamed there (user policy: improvements belong in the omabridge repo, then pin the rev).

### 4.2 Cargo features (proposed in omabridge)

```toml
[features]
default = ["cli"]                      # keeps today's `cargo build` behaviour
cli = ["dep:clap", "dep:qrcode"]       # the omabridge binary and its subcommands
embed = ["control-socket", "t3-pair", "catalog", "drafts", "remote-manage"]
control-socket = []
t3-pair = []
catalog = ["dep:t3-protocol"]          # bridge_catalog::{publish,reconcile}
drafts = []
remote-manage = []
delegated-enrollment = ["dep:t3-protocol"]
gateway = ["dep:tokio-rustls", "dep:rustls-pemfile", "dep:x509-parser"]
relay-host = ["gateway", "dep:iroh-relay"]   # iroh-relay's `server` feature
mcp = ["dep:hyper", "dep:hyper-util", "dep:http-body-util", "dep:reqwest"]
egress = []
systemd = []                           # units.rs, Systemctl, the unit-file ServeBackend
gui = ["systemd", "dep:eframe"]
```

o3-server default: `embed` plus `gateway` only on hosts that publish a public HTTPS name. `relay-host`, `mcp`, `egress`, `gui` and `systemd` stay off. The Android/desktop-visible `FEATURES` list must be derived from the enabled set (`manage::FEATURES` today is a constant).

Code changes this needs, in priority order:
1. `Target::Local` plus an `in_process_service` hook in `network.rs` (4.3).
2. `T3Issuer` -> trait (`PairingIssuer`) and `start_with_t3_issuer` taking `Arc<dyn PairingIssuer>`.
3. Per-subsystem toggles on `Node` (a `NodeConfig` struct: `control_socket`, `gateways`, `relay_host`, `sync`) instead of unconditional `tokio::spawn`; return a handle to reload config without polling.
4. `Manager` -> `ServeBackend` trait, with `UnitsBackend` (today) and `EmbeddedBackend`.
5. Make `rollback_due` take the backend, not `Systemctl`.
6. Take `HOME`/XDG from an explicit `Paths` struct for hosting/state.

### 4.3 Runtime and bypassing loopback TCP

**Shared tokio runtime.** `Node::start*` just calls `tokio::spawn`, so it runs on whatever runtime o3-server owns. Build one runtime for the process and size it (`worker_threads(2)`, `max_blocking_threads(8..16)`, `thread_stack_size(512 KiB)`) rather than `#[tokio::main]` defaults (24 workers here). The bridge needs no dedicated runtime. Add `signal`/`process` tokio features only if the binary needs them (the lib uses `process` for the CLI issuers, which are removed in the embedded path). `Store`'s blocking file I/O belongs on the blocking pool (4.2).

**Direct service dispatch.** Today the host side of a stream is `iroh stream <-> Pipe::transfer <-> TCP socket <-> loopback listener <-> hyper`. Replace the middle with an in-memory duplex:

```rust
pub trait LocalService: Send + Sync + 'static {
    /// Serve one raw byte stream (HTTP/1.1 incl. Upgrade/WebSocket) until close.
    fn serve(self: Arc<Self>, io: tokio::io::DuplexStream, peer: PeerInfo) -> BoxFuture<'static, ()>;
}
pub enum Target { Tcp(SocketAddr), Local(Arc<dyn LocalService>) }
```

In `Request::Open`, where the code does `TcpStream::connect(target)`:

```rust
let (ours, theirs) = tokio::io::duplex(64 * 1024);
tokio::spawn(service.clone().serve(theirs, peer));
pipe.transfer(ours).await   // already generic over AsyncRead+AsyncWrite
```

o3-server then implements `LocalService` with the same hyper `serve_connection(...).with_upgrades()` it uses for its TCP listener, sharing one router/state (`auto::Builder` or `http1::Builder`). Properties:
- Streams are the raw bytes the T3 HTTP/WS server sees today, so `GET /.well-known/t3/environment`, `/oauth/token` and `/ws` need no changes. Unmodified clients cannot tell the difference.
- It removes the loopback `connect`, the listener-side accept, two kernel copies per direction and the ephemeral port, and removes the "T3 port changed" class of failures (`--t3-runtime`, `omabridge_t3_unavailable`).
- The `PeerInfo` passed in (iroh `remote_id`, service, `public` flag) lets the server attribute bridge traffic (e.g. skip rate limits tied to 127.0.0.1, log the device id).
- The 1 s grant recheck stays in omabridge; it drops the duplex on revoke and hyper sees EOF.
- Duplex buffer is per direction (`max_buf_size`), allocated lazily; 64 KiB x 2 per stream, so 100 concurrent streams is about 13 MB worst case. Consider 16-32 KiB if the memory budget is tight.
- Tcp targets stay supported for `ssh` (`127.0.0.1:22`) and `mcp` services, which are other processes.
- The `compress_t3` option wraps `Pipe::transfer`'s bytes and still works unchanged on the duplex.

Alternatively, if the omabridge change is delayed, keep one loopback listener bound to `127.0.0.1:<port>` shared with the normal server (zero omabridge changes; `ServiceTargets.fixed = {"t3": addr}`). This costs only the TCP hop and is the safe first milestone. Everything else in this note is additive.

**Unix control socket.** Keep `<state_dir>/control.sock` (0600). It is how `omabridge forward/connect/t3-pair/manage` and the o3 desktop's `wait_for_server` find the running server, and it is not a TCP port. The client-side `omabridge` CLI can stay a separate small binary (built with `cli` only, no `embed`), as it is needed by the desktop on devices that are not servers.

### 4.4 State and migration

- **Use the same state dir** as today (`~/.local/share/omabridge`), so identity, peers and grants carry over and every paired phone/desktop keeps working. The `runtime.lock` rule means the old `omabridge-serve.service` **must be stopped** before o3-server starts; the migration step is "disable unit, start o3-server". Do not run both.
- `manage publish/unpublish` in embedded mode should update a `services.json` (written under the state dir with the same private-write helper, `write_document`) and the host reloads in-process. The `omabridge-serve.service` unit is then never created.
- The t3 environment id the bridge hands to peers (catalog, `T3Pairing.environment_id`) must be the one in `userdata/environment-id`. Pass it to the embedded issuer; do not make omabridge re-read the file.

### 4.5 Suggested staging

1. **M1 (no omabridge change).** Pin the omabridge rev as a dependency. Start `Node::start_reserved` with `t3 = 127.0.0.1:<port>` of o3-server's own listener and `NetOptions{ relay: Disabled|Public, bind }`. Create the control socket (comes with `Node`). Measured: +about 0.4 MB heap, about 5 MB code. All CLI and desktop behaviours continue to work. Use `T3Issuer::detect()` only if a `t3` CLI exists, otherwise `T3Pair` is denied (acceptable stopgap).
2. **M2.** Upstream `PairingIssuer` trait + `NodeConfig` toggles (control socket, gateways, relay-host, sync). Embedded issuer calls o3-server's auth directly (no subprocess, no loopback HTTP self-check).
3. **M3.** `Target::Local` duplex dispatch to o3-server's hyper service.
4. **M4.** `ServeBackend` trait so `omabridge manage` (and remote mobile admin) publish/unpublish/restart work with no systemd. Feature flags above; trim `FEATURES`.
5. **M5.** Replace the server's `omabridge manage {"action":"endpoints"}` subprocess with a direct call to `Store::service_routes` and serve `directEndpoints[kind="omabridge"]` natively.

### 4.6 Risks and open questions

- **Upstream cadence.** Memory file notes active work on `main` (PR #3 and the shared-drafts retention). Pin a rev and re-check `Cargo.lock` on each bump. The user also wants improvements to land upstream as PRs but not to be opened without explicit approval (vault decision 2026-10-26); this note proposes patches, it does not publish them.
- **iroh version coupling.** iroh `=1.4.1` and `noq` 1.4 are exact-pinned; any other iroh user in o3-server must match.
- **Blocking `Store` I/O in async paths** (`peers()` once per second per stream): fine for personal scale; revisit if hundreds of streams.
- **Single-process failure domain.** A bridge panic or stall now sits in the server process. Run the accept loop on its own tasks with `catch_unwind` at connection granularity (`JoinSet` already isolates panics per task) and keep the 64-permit cap.
- **Relay measurement gap.** Public-relay RSS and behaviour were not measured here.
- **Security surface.** The data path now terminates inside the server: the grant check, the `public` flag handling and the 1 s revoke check must be kept in omabridge (not reimplemented in the server), and the `LocalService` must still require its own bearer/session auth for non-descriptor paths exactly as for TCP. The bridge authenticates the *device*, not the T3 *session*.
- **`T3Pair` label contract.** Pairings are labelled `omabridge:<peer>` and revoked when a grant is withdrawn (`omabridge_share.rs:240` revokes by label from the desktop side). An in-process issuer must preserve the labels and the session listing/revocation API those clients use.

---

## Appendix A. Commands used

```text
CARGO_TARGET_DIR=/tmp/omab-target cargo build --release --locked --bin omabridge     # 23.7 MB, 2m53s
HOME=/tmp/omab-run/h omabridge --state-dir <a|b> --name <a|b> --relay none --bind 127.0.0.1:0 init
omabridge ... a invite --service t3 --output invite.json ; b pair --invite-file invite.json ; a approve <b-id> ; b pair (again, to learn a's address)
omabridge ... a serve --service t3=127.0.0.1:38773      # python3 -I -m http.server 38773 --bind 127.0.0.1 as the fake T3
omabridge ... b forward --peer <a-id> --service t3 --listen 127.0.0.1:38774
TOKIO_WORKER_THREADS=N MALLOC_ARENA_MAX=1 ...          # tuning
/tmp/omab-baseline (iroh Endpoint only, same features, Cargo.lock copied from omabridge)
```

Scripts and logs live only under `/tmp/omab-run`, `/tmp/omab-scripts`, `/tmp/omab-baseline`, `/tmp/omab-target`; all test processes were stopped afterwards.
