use std::net::SocketAddr;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("O3_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3773);
    let host = std::env::var("O3_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let addr: SocketAddr = format!("{host}:{port}").parse().map_err(std::io::Error::other)?;
    // One thread: idle cost is what we optimise, and every handler is non-blocking.
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        eprintln!("o3-server listening on {}", listener.local_addr()?);
        o3_server::serve(listener).await
    })
}
