use std::{net::SocketAddr, path::PathBuf, rc::Rc};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn state_dir() -> PathBuf {
    std::env::var_os("O3_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".o3/userdata")
    })
}

fn main() -> std::io::Result<()> {
    let state = o3_server::store::State::open(&state_dir()).map_err(std::io::Error::other)?;
    if std::env::args().nth(1).as_deref() == Some("pair") {
        // A short-lived one-use secret, printed once for the operator.
        let grant = state.issue_pairing(Some("cli"), 5 * 60 * 1000).map_err(std::io::Error::other)?;
        println!("{}", grant["credential"].as_str().unwrap_or_default());
        return Ok(());
    }
    let port: u16 = std::env::var("O3_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3773);
    let host = std::env::var("O3_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let addr: SocketAddr = format!("{host}:{port}").parse().map_err(std::io::Error::other)?;
    // One thread: idle cost is what we optimise, and every handler is non-blocking.
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        eprintln!("o3-server listening on {}", listener.local_addr()?);
        o3_server::serve(listener, Rc::new(state)).await
    })
}
