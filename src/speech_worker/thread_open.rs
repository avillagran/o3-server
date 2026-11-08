//! Pure original search computation using the existing admitted runtime capability.
use super::{process::OwnedProcess, Cleanup, NodeRuntime};
use serde_json::Value;
use std::{io, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
const ORIGINAL: &str = include_str!("../../runtime/search/NativeSearchRuntime.cjs");
const ENTRY: &str = r#"
const native=module.exports;
const held=setInterval(()=>{},3600000);
process.on('SIGTERM',()=>{clearInterval(held);process.exit(0)});
try {
 const input=JSON.parse(require('node:fs').readFileSync(0,'utf8'));
 if(input.op!=='recordOpen')throw Error('Invalid operation');
 const result=native.recordThreadOpen(input.previous,input.at,input.query);
 process.stdout.write(JSON.stringify({result})+'\n');
} catch (_) { clearInterval(held);process.exitCode=1; }
"#;
impl NodeRuntime {
    /// No path, code or module authority is accepted from a request.
    pub(crate) fn record_thread_open(self: &Arc<Self>, directory: &Path, input: Value) -> io::Result<Value> {
        let node=self.clone();let directory=directory.to_owned();
        std::thread::scope(|scope| scope.spawn(move || {
            let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            runtime.block_on(async move {
                if !node.file.is_executable()? {return Err(io::Error::other("Unadmitted search runtime"));}
                let mut command=tokio::process::Command::new(node.file.path());
                command.args(["--no-addons","--disable-proto=throw","--max-old-space-size=64","-e"])
                    .arg(format!("{ORIGINAL}\n{ENTRY}"))
                    .current_dir(directory).env_clear().env("PATH","").env("HOME","/nonexistent")
                    .env("NODE_OPTIONS","").env("NODE_PATH","");
                let mut process=OwnedProcess::spawn(&mut command)?;
                let (mut stdin,stdout)=match process.take_pipes(){
                    Ok(pipes)=>pipes,
                    Err(error)=>{let _=process.close(async {false},Duration::from_secs(2)).await;return Err(error);}
                };
                let mut output=BufReader::new(stdout).take(65537);
                let operation=async {
                    let bytes=serde_json::to_vec(&input)?;
                    if bytes.len()>65536{return Err(io::Error::other("Oversized search input"));}
                    stdin.write_all(&bytes).await?;stdin.shutdown().await?;
                    let mut line=Vec::new();
                    output.read_until(b'\n',&mut line).await?;
                    if line.len()>65536||line.last()!=Some(&b'\n'){return Err(io::Error::other("Invalid search response"));}
                    let row:Value=serde_json::from_slice(&line)?;
                    if row.as_object().is_none_or(|v|v.len()!=1)||!row["result"].is_object(){return Err(io::Error::other("Invalid search result"));}
                    Ok(row["result"].clone())
                };
                let result=tokio::time::timeout(Duration::from_secs(2),operation).await
                    .map_err(|_|io::Error::other("Search computation timed out")).and_then(|v|v);
                drop(stdin);
                // The owner sends TERM; actual stdout EOF from the installed
                // handler is the grace receipt, never a fabricated immediate ACK.
                let cleanup=process.close(async {let mut byte=[0u8;1];matches!(output.read(&mut byte).await,Ok(0))},Duration::from_secs(2)).await;
                if cleanup!= (Cleanup::Confirmed {graceful:true}) {return Err(io::Error::other("Search owner cleanup unconfirmed"));}
                result
            })
        }).join().map_err(|_|io::Error::other("Search owner failed"))?)
    }
}
