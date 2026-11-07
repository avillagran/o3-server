//! Owned, non-destructive acquisition of pinned device packages.
use crate::device_native_tools::{self as tools, InstalledTool, Tool};
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

fn failure(tool: Tool, step: &str) -> Value {
    json!({"_tag":"DeviceToolchainInstallError","tool":tool.package(),"step":step,"message":format!("Installing {} failed while {step}.",tool.package())})
}
struct Transaction {
    root: PathBuf,
    payload: PathBuf,
    _lock: fs::File,
}
impl Transaction {
    fn begin(base: &Path, tool: Tool) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        tools::real_directory(base)?;
        let parent = base.join("tools").join(tool.package());
        for path in [base.join("tools"), parent.clone()] {
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            tools::real_directory(&path)?;
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
                return Err(io::Error::other(
                    "Acquisition directories must be owned and not writable by others",
                ));
            }
        }
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(parent.join(".o3-acquire.lock"))?;
        use std::os::unix::fs::MetadataExt;
        let metadata = lock.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(io::Error::other("Invalid acquisition lock"));
        }
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let root = parent.join(format!(
            ".staging-{}",
            crate::store::random_id().map_err(io::Error::other)?
        ));
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        let payload = root.join("payload");
        fs::DirBuilder::new().mode(0o700).create(&payload)?;
        Ok(Self {
            root,
            payload,
            _lock: lock,
        })
    }
    fn publish(&self, destination: &Path, tool: Tool) -> io::Result<InstalledTool> {
        InstalledTool::seal_staging(&self.payload, tool)?;
        // Refuse to overwrite any existing path, including unknown trees and links.
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(self.payload.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(target_os = "linux"))]
        let result = -1;
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        fs::File::open(
            destination
                .parent()
                .ok_or_else(|| io::Error::other("Missing parent"))?,
        )?
        .sync_all()?;
        InstalledTool::inspect(destination, tool)
    }
}
impl Drop for Transaction {
    fn drop(&mut self) {
        // Preserve all artifacts after success, cancellation or failure; never traverse a tree.
        if let Some(parent) = self.root.parent() {
            let Ok(id) = crate::store::random_id() else {
                return;
            };
            let retained = parent.join(format!(".quarantine-{id}"));
            let _ = fs::rename(&self.root, retained);
        }
    }
}
/// Existing sealed versions are reused. An incomplete destination is never replaced.
pub async fn ensure(base: &Path, tool: Tool, npm: &Path) -> Result<InstalledTool, Value> {
    let destination = base.join("tools").join(tool.package()).join(tool.version());
    if let Ok(installed) = InstalledTool::inspect(&destination, tool) {
        return Ok(installed);
    }
    if !cfg!(target_os = "linux") {
        return Err(failure(tool, "requiring supported atomic publication"));
    }
    let transaction = Transaction::begin(base, tool)
        .map_err(|_| failure(tool, "preparing the install directory"))?;
    if let Ok(installed) = InstalledTool::inspect(&destination, tool) {
        return Ok(installed);
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(failure(tool, "preserving an existing incomplete install"));
    }
    let mut args = tools::npm_acquisition_args(&transaction.payload, tool);
    let delimiter = args
        .iter()
        .position(|arg| arg == "--")
        .expect("fixed npm argv");
    args.splice(
        delimiter..delimiter,
        [
            "--userconfig".into(),
            "/dev/null".into(),
            "--globalconfig".into(),
            "/dev/null".into(),
            "--cache".into(),
            transaction
                .root
                .join("cache")
                .to_string_lossy()
                .into_owned(),
        ],
    );
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = crate::proc::run_installer(
        npm.to_str()
            .ok_or_else(|| failure(tool, "running npm install"))?,
        &transaction.root,
        &refs,
        crate::proc::Opts {
            timeout: Duration::from_secs(600),
            cap: 1024 * 1024,
            allow_truncate: false,
        },
    )
    .await
    .map_err(|_| failure(tool, "running npm install"))?;
    if !output.ok() || output.truncated {
        let mut error = failure(tool, "running npm install");
        if let Some(code) = output.code {
            error["exitCode"] = json!(code);
            error["message"] = json!(format!(
                "Installing {} failed while running npm install (exit code {code}).",
                tool.package()
            ));
        }
        return Err(error);
    }
    tokio::task::spawn_blocking(move || transaction.publish(&destination, tool))
        .await
        .map_err(|_| failure(tool, "verifying and publishing the installed inventory"))?
        .map_err(|_| failure(tool, "verifying and publishing the installed inventory"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    pub(crate) fn fixture(mode: &str) -> (PathBuf, PathBuf) {
        crate::test_sandbox::assert_isolated();
        let base = std::env::temp_dir().join(format!(
            "device-acquire-{}",
            crate::store::random_id().unwrap()
        ));
        fs::create_dir(&base).unwrap();
        let npm = base.join("private-npm");
        let program = format!(
            r#"#!/usr/bin/python3
import sys,pathlib,json,os
args=sys.argv[1:]
p=pathlib.Path(args[args.index('--prefix')+1]);mode={mode:?}
assert '--ignore-scripts' in args and '--no-bin-links' in args
assert args[args.index('--userconfig')+1]=='/dev/null'
assert 'NODE_OPTIONS' not in os.environ
if mode=='failure':
 print('https://private:credential@example.test/package',file=sys.stderr);sys.exit(1)
name,version=args[-1].rsplit('@',1);pkg=p/'node_modules'/name
entry=pkg/('dist/server/cli.mjs' if name=='expo-device-hub' else 'bin/agent-device.mjs')
entry.parent.mkdir(parents=True)
(pkg/'package.json').write_text(json.dumps({{'name':name,'version':version}}))
entry.write_text('// Synthetic package; never executed')
if mode=='hub-server':
 entry.write_text("import http from 'node:http'; const port=Number(process.argv[process.argv.indexOf('--port')+1]); http.createServer((req,res)=>{{if(req.url==='/readyz'){{res.end('ok');}}else{{res.setHeader('content-type','application/json');res.end(JSON.stringify({{simulators:[],emulators:[]}}));}}}}).listen(port,'127.0.0.1');")
if mode=='link': (p/'linked').symlink_to(p/'node_modules')
if mode=='fifo': os.mkfifo(p/'pipe')
if mode=='wrong': (pkg/'package.json').write_text('{{}}')
"#
        );
        fs::write(&npm, program).unwrap();
        fs::set_permissions(&npm, fs::Permissions::from_mode(0o700)).unwrap();
        (base, npm)
    }
    fn archive() -> Vec<u8> {
        let mut output = Vec::new();
        for (name, data) in [
            (
                "node_modules/expo-device-hub/package.json",
                br#"{"name":"expo-device-hub","version":"0.19.1"}"#.as_slice(),
            ),
            (
                "node_modules/expo-device-hub/dist/server/cli.mjs",
                b"// Synthetic archive entry; never executed".as_slice(),
            ),
        ] {
            let mut header = [0_u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            header[100..108].copy_from_slice(b"0000600\0");
            header[108..116].copy_from_slice(b"0000000\0");
            header[116..124].copy_from_slice(b"0000000\0");
            let size = format!("{:011o}\0", data.len());
            header[124..136].copy_from_slice(size.as_bytes());
            header[136..148].copy_from_slice(b"00000000000\0");
            header[148..156].fill(b' ');
            header[156] = b'0';
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum = header.iter().map(|byte| u32::from(*byte)).sum::<u32>();
            header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
            output.extend(header);
            output.extend(data);
            output.resize(output.len().next_multiple_of(512), 0);
        }
        output.resize(output.len() + 1024, 0);
        output
    }
    #[tokio::test]
    async fn private_http_archive_is_fully_downloaded_before_inventory_publication() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (base, npm) = fixture("ok");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let bytes = archive();
        let serving = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).await.unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count]).starts_with("GET /pinned-fixture.tar ")
            );
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&bytes).await.unwrap();
        });
        let program = format!(
            r#"#!/usr/bin/python3
import sys,pathlib,urllib.request,tarfile,io
args=sys.argv[1:];p=pathlib.Path(args[args.index('--prefix')+1])
assert 'expo-device-hub@0.19.1' in args and '--ignore-scripts' in args
with urllib.request.urlopen('http://{address}/pinned-fixture.tar',timeout=5) as response: archive=response.read(1048576)
with tarfile.open(fileobj=io.BytesIO(archive),mode='r:') as tar:
 for item in tar.getmembers():
  name=pathlib.PurePosixPath(item.name)
  assert item.isfile() and not name.is_absolute() and '..' not in name.parts
  target=p.joinpath(*name.parts);target.parent.mkdir(parents=True,exist_ok=True)
  with target.open('xb') as output: output.write(tar.extractfile(item).read(1048576))
"#
        );
        fs::write(&npm, program).unwrap();
        let installed = ensure(&base, Tool::Hub, &npm).await.unwrap();
        serving.await.unwrap();
        InstalledTool::inspect(&installed.root, Tool::Hub).unwrap();
    }
    #[test]
    fn native_pins_paths_and_install_argv_are_source_backed() {
        crate::test_sandbox::assert_isolated();
        let corpus: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-acquire-4288.json"
        ))
        .unwrap();
        assert_eq!(corpus["cases"].as_array().unwrap().len(), 24);
        for case in corpus["cases"].as_array().unwrap() {
            let tool = if case["input"]["tool"] == "hub" {
                Tool::Hub
            } else {
                Tool::Agent
            };
            for command in case["expected"]["commands"].as_array().unwrap() {
                assert_eq!(command["command"], "npm");
                assert_eq!(command["timeoutMilliseconds"], 600000);
                let args = command["args"].as_array().unwrap();
                assert_eq!(
                    args.last().unwrap(),
                    &json!(format!("{}@{}", tool.package(), tool.version()))
                );
                assert_eq!(args[0], "install");
                assert_eq!(args[1], "--prefix");
                let safe = tools::npm_acquisition_args(Path::new("/private/staging"), tool);
                assert!(safe.contains(&"--ignore-scripts".into()));
                assert!(safe.contains(&"--no-bin-links".into()));
            }
            if let Some(paths) = case["expected"].get("paths") {
                assert!(paths["installDir"].as_str().unwrap().ends_with(&format!(
                    "{}/{}",
                    tool.package(),
                    tool.version()
                )));
                assert!(paths["entryPath"].as_str().unwrap().ends_with(tool.entry()));
            }
        }
    }
    #[tokio::test]
    async fn private_process_publishes_once_and_reuses_complete_inventory() {
        let (base, npm) = fixture("ok");
        let installed = ensure(&base, Tool::Hub, &npm).await.unwrap();
        assert!(installed.entry.is_file());
        fs::write(&npm, "invalid executable; must not run again").unwrap();
        assert_eq!(
            ensure(&base, Tool::Hub, &npm).await.unwrap().root,
            installed.root
        );
        assert!(fs::read_dir(installed.root.parent().unwrap())
            .unwrap()
            .any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".quarantine-")));
    }
    #[tokio::test]
    async fn failed_and_hostile_results_are_retained_without_publishing_or_disclosing_output() {
        for mode in ["failure", "link", "fifo", "wrong"] {
            let (base, npm) = fixture(mode);
            let error = ensure(&base, Tool::Hub, &npm).await.unwrap_err();
            assert!(!error.to_string().contains("credential"));
            let parent = base.join("tools/expo-device-hub");
            assert!(!parent.join("0.19.1").exists());
            let retained = fs::read_dir(parent)
                .unwrap()
                .filter_map(Result::ok)
                .find(|e| e.file_name().to_string_lossy().starts_with(".quarantine-"))
                .unwrap();
            assert!(retained.path().join("payload").is_dir());
        }
    }
    #[test]
    fn concurrent_transaction_and_foreign_writable_directory_are_refused() {
        let (base, _) = fixture("ok");
        let transaction = Transaction::begin(&base, Tool::Hub).unwrap();
        assert!(Transaction::begin(&base, Tool::Hub).is_err());
        drop(transaction);
        let next = Transaction::begin(&base, Tool::Hub).unwrap();
        drop(next);
        fs::set_permissions(base.join("tools"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Transaction::begin(&base, Tool::Hub).is_err());
    }
    #[test]
    fn publication_race_cannot_overwrite_a_new_unknown_destination() {
        let (base, _) = fixture("ok");
        let transaction = Transaction::begin(&base, Tool::Hub).unwrap();
        let package = transaction.payload.join("node_modules/expo-device-hub");
        fs::create_dir_all(package.join("dist/server")).unwrap();
        fs::write(
            package.join("package.json"),
            br#"{"name":"expo-device-hub","version":"0.19.1"}"#,
        )
        .unwrap();
        fs::write(package.join("dist/server/cli.mjs"), "// private fixture").unwrap();
        let destination = base.join("tools/expo-device-hub/0.19.1");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("kept"), "unknown owned data").unwrap();
        assert!(transaction.publish(&destination, Tool::Hub).is_err());
        assert_eq!(
            fs::read_to_string(destination.join("kept")).unwrap(),
            "unknown owned data"
        );
        assert!(transaction
            .payload
            .join(".o3-device-install.json")
            .is_file());
    }
    #[tokio::test]
    async fn existing_unknown_tree_and_link_are_never_replaced() {
        for link in [false, true] {
            let (base, npm) = fixture("ok");
            let parent = base.join("tools/expo-device-hub");
            fs::create_dir_all(&parent).unwrap();
            let target = base.join("private-target");
            fs::create_dir(&target).unwrap();
            fs::write(target.join("kept"), "unchanged").unwrap();
            let destination = parent.join("0.19.1");
            if link {
                std::os::unix::fs::symlink(&target, &destination).unwrap();
            } else {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("kept"), "unchanged").unwrap();
            }
            assert!(ensure(&base, Tool::Hub, &npm).await.is_err());
            assert_eq!(
                fs::read_to_string(target.join("kept")).unwrap(),
                "unchanged"
            );
            assert_eq!(
                fs::read_to_string(destination.join("kept")).unwrap(),
                "unchanged"
            );
        }
    }
}
