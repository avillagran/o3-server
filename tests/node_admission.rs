//! The runner supplies a sealed executable receipt rather than an ambient Node command.
#[path = "support/sandbox.rs"]
mod sandbox;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Command,
};

fn digest(file: &mut File) -> String {
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut hash = Sha256::new();
    let mut block = vec![0; 64 * 1024];
    loop {
        let count = file.read(&mut block).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&block[..count]);
    }
    format!("{:x}", hash.finalize())
}
#[test]
fn admitted_node_receipt_reaches_private_runtime_as_exact_binary() {
    sandbox::assert_isolated();
    let node = PathBuf::from(
        std::env::var("O3_ADMITTED_NODE")
            .expect("Private harness must supply an admitted Node path"),
    );
    let expected = std::env::var("O3_ADMITTED_NODE_SHA256")
        .expect("Private harness must supply its Node SHA-256");
    let version = std::env::var("O3_ADMITTED_NODE_VERSION")
        .expect("Private harness must supply its Node version");
    assert_eq!(version, "26.13.0");
    assert!(node.is_absolute());
    assert_eq!(expected.len(), 64);
    assert!(expected
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&node)
        .unwrap();
    let before = file.metadata().unwrap();
    assert!(before.is_file());
    assert_ne!(before.permissions().mode() & 0o111, 0);
    assert_eq!(before.permissions().mode() & 0o6000, 0);
    assert_eq!(digest(&mut file), expected);
    let home = std::env::temp_dir().join(format!(
        "o3-node-receipt-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&home)
        .unwrap();
    for (args, text) in [
        (vec!["--version"], format!("v{version}\n")),
        (
            vec!["-p", "process.execPath"],
            format!("{}\n", node.display()),
        ),
    ] {
        let output = Command::new(&node)
            .args(args)
            .env_clear()
            .env("HOME", &home)
            .env("TMPDIR", &home)
            .env("PATH", "")
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), text);
        assert!(output.stderr.is_empty());
    }
    let after = file.metadata().unwrap();
    assert_eq!(
        (
            before.dev(),
            before.ino(),
            before.len(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec()
        ),
        (
            after.dev(),
            after.ino(),
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec()
        )
    );
    assert_eq!(digest(&mut file), expected);
    // The private home remains retained; no fixture tree is removed.
}
