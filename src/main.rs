use std::{path::Path, sync::Arc};
#[cfg(feature = "memory-diagnostics")]
mod memory;
#[cfg(feature = "memory-diagnostics")]
#[global_allocator]
static MEASURED: memory::Measured = memory::Measured;
#[cfg(not(feature = "memory-diagnostics"))]
#[global_allocator]
static GLOBAL: std::alloc::System = std::alloc::System;
const USAGE: &str = "\
Usage: o3-server [COMMAND] [OPTIONS]

Commands:
  serve                     Run the server (default)
  import --from PATH        Import a T3 statev2.sqlite, read-only
  auth session issue        Issue a bearer session
  auth mcp issue            Issue a dedicated MCP credential (--access read-only by default)
  auth session list         List sessions
  auth session revoke ID    Revoke a session
  auth pairing create       Create a one-time pairing credential
  service install           Install the user service
  shared-profile-readiness  Inspect legacy webhook handover without changing the profile
  --version                 Print the version

Native search runtime (installed runtime/node-26.13.0, no PATH discovery):
  --native-search-node PATH Exact admitted Node for thread open history

Local speech startup (optional, no PATH discovery):
  --speech-node PATH        Exact admitted Node 26.13.0 executable
  --speech-bundle DIR       Installed pinned engine bundle (contains graph/)

The profile comes from T3CODE_HOME (default ~/.t3).
";
fn option<'a>(a: &'a [String], name: &str) -> Option<&'a str> {
    a.windows(2).find(|w| w[0] == name).map(|w| w[1].as_str())
}
fn ttl(s: &str) -> std::io::Result<i64> {
    let (n, m) = if let Some(v) = s.strip_suffix('d') {
        (v, 86400000)
    } else if let Some(v) = s.strip_suffix('h') {
        (v, 3600000)
    } else if let Some(v) = s.strip_suffix('m') {
        (v, 60000)
    } else {
        (s, 1000)
    };
    n.parse::<i64>()
        .ok()
        .and_then(|n| n.checked_mul(m))
        .filter(|n| *n > 0)
        .ok_or_else(|| std::io::Error::other("invalid TTL"))
}
fn main() -> std::io::Result<()> {
    let launcher_parent = o3_server::desktop_startup::parent_id();
    o3_server::heap::configure();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "__document-text") {
        if args.len() != 3 {
            std::process::exit(2);
        }
        o3_server::document_text::worker(Path::new(&args[1]), &args[2]);
    }
    // Installers verify a downloaded binary with this; it touches no profile.
    if args.first().is_some_and(|s| s == "--version" || s == "-V") {
        println!("o3-server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // Before any command runs: `auth session issue --help` must never mint a credential.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return Ok(());
    }
    if let Some(code) = o3_server::shared_readiness::run_cli(&args) {
        std::process::exit(code);
    }
    if args.first().map(String::as_str) == Some("__ssh-helper") {
        match args.get(1).map(String::as_str) {
            Some("runtime-port") => {
                let v: serde_json::Value = serde_json::from_slice(&std::fs::read(
                    args.get(2)
                        .ok_or_else(|| std::io::Error::other("runtime path required"))?,
                )?)?;
                println!("{} {}", v["pid"], v["port"]);
                return Ok(());
            }
            Some("wait-ready") => {
                use std::io::{Read, Write};
                let port = args
                    .get(2)
                    .ok_or_else(|| std::io::Error::other("port required"))?;
                let timeout = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(15000);
                let interval = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1000u64);
                let start = std::time::Instant::now();
                while start.elapsed().as_millis() < (timeout as u128) {
                    if let Ok(mut s) = std::net::TcpStream::connect(format!("127.0.0.1:{port}")) {
                        s.set_read_timeout(Some(std::time::Duration::from_secs(1)))?;
                        s.write_all(
                            b"GET /.well-known/t3/environment HTTP/1.0\r\nHost: localhost\r\n\r\n",
                        )?;
                        let mut b = [0; 32];
                        if s.read(&mut b)
                            .is_ok_and(|n| String::from_utf8_lossy(&b[..n]).contains("200"))
                        {
                            return Ok(());
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(interval.max(1)));
                }
                return Err(std::io::Error::other("server readiness timed out"));
            }
            _ => return Err(std::io::Error::other("unknown ssh helper")),
        }
    }
    #[cfg(feature = "bridge")]
    if args.iter().any(|a| {
        matches!(
            a.as_str(),
            "manage" | "peers" | "forward" | "forwards" | "t3-pair"
        )
    }) && option(&args, "--state-dir").is_some()
    {
        return o3_server::bridge::cli(&args);
    }
    let is_serving = args
        .first()
        .is_none_or(|arg| matches!(arg.as_str(), "serve" | "start") || arg.starts_with('-'));
    let speech_config = if is_serving {
        o3_server::speech_runtime::Config::from_args(&args).map_err(std::io::Error::other)?
    } else {
        None
    };
    let mut startup = if is_serving {
        Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on({
                    // SAFETY: this is the sole startup reader, before providers or
                    // profile discovery; the trusted launcher transfers its channels.
                    unsafe { o3_server::desktop_startup::Startup::read(&args) }
                })?,
        )
    } else {
        None
    };
    let bootstrap = startup
        .as_ref()
        .and_then(|startup| startup.envelope.as_ref());
    let desktop_mode = startup.as_ref().is_some_and(|startup| startup.desktop);
    let watch_launcher = startup
        .as_ref()
        .is_some_and(|startup| startup.parent_watch_enabled(launcher_parent));
    let mut profile_args = args.clone();
    let explicit_home = option(&args, "--base-dir")
        .map(str::to_owned)
        .or_else(|| std::env::var("T3CODE_HOME").ok());
    if explicit_home
        .as_deref()
        .is_none_or(|home| home.trim().is_empty())
    {
        if let Some(home) = bootstrap.and_then(|bootstrap| bootstrap.home()) {
            if let Some(index) = profile_args
                .windows(2)
                .position(|pair| pair[0] == "--base-dir")
            {
                profile_args[index + 1] = home.into();
            } else {
                profile_args.extend(["--base-dir".into(), home.into()]);
            }
        }
    }
    let base = o3_server::profile::base_dir(&profile_args)?;
    let dir = base.join("userdata");
    if args.starts_with(&["service".into(), "install".into()]) {
        return o3_server::service::install(&base, args.iter().any(|s| s == "--dry-run"));
    }
    if args.first().is_some_and(|s| {
        !matches!(s.as_str(), "serve" | "start" | "auth" | "import") && !s.starts_with('-')
    }) {
        return Err(std::io::Error::other("unsupported command"));
    }
    // Before anything creates o3 files, which would pollute another server's profile.
    o3_server::profile::refuse_foreign_live(Path::new(&dir))?;
    let policy = match o3_server::managed_policy::Policy::load_server() {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(o3_server::managed_policy::EXIT_CODE);
        }
    };
    let provider_policy = policy.clone();
    let is_auth = args.first().map(String::as_str) == Some("auth");
    let port = option(&args, "--port")
        .map(str::to_owned)
        .or_else(|| std::env::var("T3CODE_PORT").ok())
        .or_else(|| bootstrap.map(|bootstrap| bootstrap.port().to_string()))
        .unwrap_or("3773".into());
    let mut host = option(&args, "--host")
        .map(str::to_owned)
        .or_else(|| std::env::var("T3CODE_HOST").ok())
        .or_else(|| bootstrap.map(|bootstrap| bootstrap.host().to_owned()))
        .unwrap_or("127.0.0.1".into());
    if is_serving {
        let port_number: u16 = port.parse().map_err(std::io::Error::other)?;
        let reachability = o3_server::managed_limits::Reachability {
            auth_policy: o3_server::desktop_startup::auth_policy(Some(&host), desktop_mode),
            host: Some(host.clone()),
            tailscale_serve_enabled: false,
            private_network_only: None,
        };
        host =
            o3_server::managed_limits::check_reachability(policy.document.as_ref(), &reachability)
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(o3_server::managed_policy::EXIT_CODE);
                });
        if !policy.allows_remote() {
            if port_number == 0 {
                eprintln!("A managed local listener requires a fixed nonzero port");
                std::process::exit(o3_server::managed_policy::EXIT_CODE);
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime
                .block_on(o3_server::managed_tailscale_guard::enforce(
                    &policy,
                    &o3_server::managed_tailscale::Target {
                        host: host.clone(),
                        port: port_number,
                    },
                ))
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(o3_server::managed_policy::EXIT_CODE);
                });
        }
    }
    let _ownership = if !is_auth {
        Some(o3_server::profile::ownership(&dir)?)
    } else {
        None
    };
    let mut state = o3_server::store::State::open_with_policy(Path::new(&dir), policy)
        .map_err(std::io::Error::other)?;
    state.asset_profile_root = base.clone();
    if is_serving {
        state
            .configure_listener_auth(
                Some(&host),
                desktop_mode,
                bootstrap.map(|bootstrap| bootstrap.token().to_owned()),
                bootstrap
                    .and_then(|bootstrap| bootstrap.secret())
                    .map(str::to_owned),
            )
            .map_err(std::io::Error::other)?;
    }
    if args.first().map(String::as_str) == Some("import") {
        let source =
            option(&args, "--from").ok_or_else(|| std::io::Error::other("--from is required"))?;
        let mut report =
            o3_server::import::from_t3(&state, Path::new(source)).map_err(std::io::Error::other)?;
        // T3 keeps attachment files beside its database; a backup copy elsewhere needs the
        // directory named explicitly.
        let attachments = option(&args, "--attachments")
            .map(std::path::PathBuf::from)
            .or_else(|| Path::new(source).parent().map(|p| p.join("attachments")));
        if let Some(dir) = attachments.filter(|d| d.is_dir()) {
            report["attachments"] = serde_json::json!(
                o3_server::import::attachments(&state, &dir).map_err(std::io::Error::other)?
            );
        }
        println!("{report}");
        return Ok(());
    }
    if is_auth {
        if args.get(1).map(String::as_str) == Some("mcp")
            && args.get(2).map(String::as_str) == Some("issue")
        {
            let token = state
                .issue_mcp_session(
                    option(&args, "--access").unwrap_or("read-only"),
                    ttl(option(&args, "--ttl").unwrap_or("30d"))?,
                    option(&args, "--label").unwrap_or("MCP client"),
                )
                .map_err(std::io::Error::other)?;
            if args.iter().any(|s| s == "--token-only") {
                println!("{token}");
            } else {
                println!("{}", serde_json::json!({"token":token}));
            }
            return Ok(());
        }
        if args.get(1).map(String::as_str) == Some("session")
            && args.get(2).map(String::as_str) == Some("issue")
        {
            let mut scopes: Vec<String> = args
                .windows(2)
                .filter(|w| w[0] == "--scope")
                .map(|w| w[1].clone())
                .collect();
            if scopes.is_empty() {
                scopes = o3_server::store::STANDARD_SCOPES
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect();
            }
            if scopes
                .iter()
                .any(|s| !o3_server::store::STANDARD_SCOPES.contains(&s.as_str()))
            {
                return Err(std::io::Error::other("unknown scope"));
            }
            let token = state
                .issue_session(
                    option(&args, "--subject").unwrap_or("owner"),
                    &scopes,
                    ttl(option(&args, "--ttl").unwrap_or("30d"))?,
                    option(&args, "--label").unwrap_or("cli"),
                )
                .map_err(std::io::Error::other)?;
            if args.iter().any(|s| s == "--token-only") {
                println!("{token}")
            } else {
                println!("{}", serde_json::json!({"token":token}))
            };
            return Ok(());
        }
        if args.get(1).map(String::as_str) == Some("session") {
            match args.get(2).map(String::as_str) {
                Some("list") => {
                    println!("{}", state.list_sessions().map_err(std::io::Error::other)?);
                    return Ok(());
                }
                Some("revoke") => {
                    // The desktop supplies the session id as the last argument.
                    let id = args
                        .last()
                        .filter(|s| !s.starts_with('-') && args.len() > 3)
                        .ok_or_else(|| std::io::Error::other("session id required"))?;
                    state.revoke_session(id).map_err(std::io::Error::other)?;
                    return Ok(());
                }
                _ => {}
            }
        }
        if args.get(1).map(String::as_str) == Some("pairing")
            && args.get(2).map(String::as_str) == Some("create")
        {
            println!(
                "{}",
                state
                    .issue_pairing(
                        option(&args, "--label"),
                        ttl(option(&args, "--ttl").unwrap_or("5m"))?
                    )
                    .map_err(std::io::Error::other)?
            );
            return Ok(());
        }
        return Err(std::io::Error::other("unsupported auth command"));
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .on_thread_park(o3_server::heap::on_thread_park)
        .max_blocking_threads(2)
        .thread_keep_alive(std::time::Duration::from_secs(1))
        .on_thread_stop(o3_server::heap::collect)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()?
        .block_on(async {
            #[cfg(feature = "memory-diagnostics")]
            memory::start();
            state.agent_scopes.enable();
            let codex = Arc::new(
                o3_server::codex::CodexProvider::default()
                    .with_agent_scopes(state.agent_scopes.clone())
                    .with_managed_policy(provider_policy.document.is_some()),
            );
            *state.provider.lock().unwrap() = Some(Arc::new(
                o3_server::provider::ConfiguredProviders::new(
                    Arc::new(o3_server::provider::Providers {
                        claude: o3_server::claude::ClaudeProvider::discover().map(|provider| {
                            Arc::new(
                                provider
                                    .with_agent_scopes(state.agent_scopes.clone())
                                    .with_managed_policy(provider_policy.document.is_some()),
                            )
                        }),
                        codex: codex.clone(),
                    }),
                    dir.clone(),
                )
                .with_agent_scopes(state.agent_scopes.clone())
                .with_policy(provider_policy.clone())
                .with_secret_resolver(state.secret_resolver())
                .with_settings_lock(state.settings_lock.clone())
                .with_notifications(state.changed.clone()),
            ));
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let mut interrupt =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            let listener = tokio::net::TcpListener::bind((
                host.as_str(),
                port.parse::<u16>().map_err(std::io::Error::other)?,
            ))
            .await?;
            let _guard = o3_server::profile::RuntimeGuard::write(&dir, listener.local_addr()?)?;
            o3_server::desktop_startup::log_stderr(format_args!("o3-server listening on {}", listener.local_addr()?))?;
            // T3's base must be taken in before the first client reads.
            let shared = match o3_server::t3_shared::start(&state) {
                Ok(shared) => shared,
                Err(e) => {
                    o3_server::desktop_startup::log_stderr(format_args!("T3 shared base not written: {e}"))?;
                    None
                }
            };
            let state = Arc::new(state);
            o3_server::thread_open::install(&state,&args).map_err(std::io::Error::other)?;
            if let Some(config)=speech_config {
                if let Err(cause)=o3_server::speech_runtime::install(state.clone(),config).await {
                    o3_server::desktop_startup::log_stderr(format_args!("Local speech runtime not admitted: {cause}"))?;
                }
            }
            let desktop_driver = if let Some(descriptors) = startup.as_mut().and_then(|startup| startup.browser.take()) {
                Some(o3_server::preview::install_desktop(state.clone(), descriptors.into_channel()?).await)
            } else { None };
            let exporter =
                shared.map(|t3| o3_server::t3_shared::Exporter::spawn(state.clone(), t3));
            let probe = state.provider.lock().unwrap().clone().unwrap();
            let probe_state = state.clone();
            let probe_task = tokio::spawn(async move {
                let mut changes = probe_state.activity.subscribe();
                while !probe_state.background_needed() {
                    if changes.changed().await.is_err() {
                        return;
                    }
                }
                if let Err(e) = probe
                    .discover(std::env::current_dir().unwrap_or_else(|_| probe_state.dir.clone()))
                    .await
                {
                    let _ = o3_server::desktop_startup::log_stderr(format_args!("Codex unavailable: {e}"));
                }
                probe_state.notify();
            });
            let mut serving = tokio::spawn(o3_server::serve(listener, state.clone()));
            // A standalone OmaBridge can publish this server instead; --no-bridge keeps the
            // embedded node, its identity and its timers out of this process.
            #[cfg(feature = "bridge")]
            let bridge =
                if args.iter().any(|a| a == "--no-bridge") || !provider_policy.allows_remote() {
                    None
                } else {
                    Some(
                        o3_server::bridge::start_hosted(
                            &option(&args, "--bridge-state-dir")
                                .map(std::path::PathBuf::from)
                                .unwrap_or_else(|| base.join("omabridge")),
                            state.clone(),
                            {
                                let mut options = omabridge::NetOptions::loopback();
                                if let Some(bind) = option(&args, "--bridge-bind") {
                                    options.bind = bind.parse().map_err(std::io::Error::other)?;
                                }
                                if let Some(relay) = option(&args, "--bridge-relay") {
                                    options.relay = match relay {
                                        "none" => omabridge::Relay::Disabled,
                                        "public" => omabridge::Relay::Public,
                                        url => omabridge::Relay::Custom(
                                            url.parse().map_err(std::io::Error::other)?,
                                        ),
                                    };
                                }
                                options
                            },
                            option(&args, "--bridge-gateway-config").map(Path::new),
                            args.iter().any(|a| a == "--bridge-relay-host"),
                        )
                        .await
                        .map_err(std::io::Error::other)?,
                    )
                };
            // block_on runs on the main thread, outside the runtime worker park hooks.
            let result = tokio::select! {
                result=&mut serving=>result.map_err(std::io::Error::other)?,
                _=interrupt.recv()=>Ok(()),
                _=term.recv()=>Ok(()),
                _=o3_server::desktop_startup::wait_for_parent_change(launcher_parent,watch_launcher)=>Ok(()),
            };
            state.begin_shutdown();
            if !serving.is_finished() {
                serving.abort();
                let _ = serving.await;
            }
            #[cfg(feature = "bridge")]
            if let Some(bridge) = bridge {
                bridge.shutdown().await;
            }
            probe_task.abort();
            let _ = probe_task.await;
            let speech_cleanup=o3_server::speech_state::shutdown(&state).await;
            o3_server::preview::shutdown(&state).await;
            if let Some(driver) = desktop_driver { driver.shutdown().await; }
            codex.shutdown().await;
            if let Some(exporter) = exporter {
                exporter.finish();
            }
            if speech_cleanup.is_err() && result.is_ok() {Err(std::io::Error::other("Owned speech cleanup remains unknown"))}else{result}
        })
}
