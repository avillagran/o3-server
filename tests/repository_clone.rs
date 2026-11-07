mod support;
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, process::Command};
#[test]
fn repository_lookup_uses_protocol_and_only_auto_retries_ssh_rejection() {
    let root = std::env::temp_dir().join(format!("o3-clone-protocol-{}", std::process::id()));
    std::fs::create_dir_all(root.join("source")).unwrap();
    let source = root.join("source");
    let bins = root.join("bin");
    std::fs::create_dir_all(&bins).unwrap();
    for args in [
        &["init", "-b", "main"][..],
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "First",
        ][..],
    ] {
        assert!(Command::new("/usr/bin/git")
            .args(args)
            .current_dir(&source)
            .output()
            .unwrap()
            .status
            .success());
    }
    let https = format!("file://{}", source.display());
    let ssh = "git@github.com:fixture/repository.git";
    let metadata = json!({"nameWithOwner":"fixture/repository","url":https,"sshUrl":ssh});
    let gh=format!("#!/usr/bin/python3\nimport sys\nassert sys.argv[1:4]==['repo','view','fixture/repository'],sys.argv\nprint({:?})\n",metadata.to_string());
    let log = root.join("clone-argv.jsonl");
    let git=format!("#!/usr/bin/python3\nimport sys,os,json\nargs=sys.argv[1:]\nif 'clone' in args:\n with open({:?},'a') as log:log.write(json.dumps(args)+'\\n')\n if {:?} in args:\n  sys.stderr.write('Permission denied (publickey). private-stderr-marker')\n  sys.exit(128)\nif 'commit' in args and os.path.basename(os.getcwd()).startswith('no-commit'):\n sys.stderr.write('Author identity unknown')\n sys.exit(128)\nos.execv('/usr/bin/git',['git']+args)\n",log.to_str().unwrap(),ssh);
    for (name, body) in [("gh", gh), ("git", git)] {
        let p = bins.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // This integration binary has one test; install the fixture before starting server threads.
    std::env::set_var(
        "PATH",
        format!("{}:{}", bins.display(), std::env::var("PATH").unwrap()),
    );
    let s = support::start(|_| {});
    let c = s.connect();
    let auto=c.rpc("sourceControl.cloneRepository",json!({"provider":"github","repository":"fixture/repository","destinationPath":root.join("auto")})).unwrap();
    assert_eq!(auto["remoteUrl"], https);
    assert_eq!(auto["repository"]["nameWithOwner"], "fixture/repository");
    let ssh_error=c.rpc("sourceControl.cloneRepository",json!({"provider":"github","repository":"fixture/repository","protocol":"ssh","destinationPath":root.join("ssh")})).unwrap_err();
    assert!(!ssh_error.to_string().contains("private-stderr-marker"));
    c.rpc("sourceControl.cloneRepository",json!({"provider":"github","repository":"fixture/repository","protocol":"https","destinationPath":root.join("https")})).unwrap();
    let calls: Vec<Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(calls.len(), 4);
    for (call, remote) in calls.iter().zip([ssh, https.as_str(), ssh, https.as_str()]) {
        assert!(call.as_array().unwrap().iter().any(|s| s == remote));
    }
    let snapshot = c.request_json("GET", "/api/projects", None, false).unwrap();
    assert_eq!(snapshot["projects"], json!([]));
    let named = c
        .rpc(
            "projects.createNew",
            json!({"name":format!("No Commit {}",std::process::id())}),
        )
        .unwrap();
    assert!(named["commitError"].as_str().unwrap().contains("user.name"));
    let named_root = std::path::Path::new(named["workspaceRoot"].as_str().unwrap());
    assert!(named_root.join("README.md").is_file());
    let snapshot = c.request_json("GET", "/api/projects", None, false).unwrap();
    assert!(snapshot["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == named["projectId"]));
    // Fixture trees are retained for private container teardown.

    // Fixture trees are retained for private container teardown.
}
