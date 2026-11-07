//! Only synthetic stores and exact owned files are used. No recursive fixture cleanup.
#[path = "support/sandbox.rs"]
mod sandbox;
#[path = "../src/server_secret_store.rs"]
mod server_secret_store;
#[path = "../src/settings_secret_transaction.rs"]
mod settings_secret_transaction;
use o3_server::{secret_resolver, settings_secrets};
use serde_json::Value;
use server_secret_store::{FileSecretStore, SecretStore, StoreError};
use settings_secret_transaction as tx;
use std::{
    cell::RefCell,
    collections::HashMap,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    sync::{Arc, Barrier},
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/secret-store-4288.json")).unwrap()
}
#[derive(Default)]
struct Memory {
    files: RefCell<HashMap<String, Vec<u8>>>,
    fail: RefCell<Option<(&'static str, usize, bool)>>,
    additional_failures: RefCell<Vec<(&'static str, usize, bool)>>,
    counts: RefCell<HashMap<&'static str, usize>>,
    trace: RefCell<Vec<(String, String)>>,
}
impl Memory {
    fn operation<T>(
        &self,
        kind: &'static str,
        name: &str,
        run: impl FnOnce() -> T,
    ) -> Result<T, StoreError> {
        self.trace.borrow_mut().push((kind.into(), name.into()));
        let count = {
            let mut c = self.counts.borrow_mut();
            let v = c.entry(kind).or_default();
            *v += 1;
            *v
        };
        let fail = self
            .fail
            .borrow()
            .filter(|(k, n, _)| *k == kind && *n == count)
            .or_else(|| {
                self.additional_failures
                    .borrow()
                    .iter()
                    .copied()
                    .find(|(k, n, _)| *k == kind && *n == count)
            });
        if fail.is_some_and(|(_, _, after)| !after) {
            return Err(StoreError::new(
                match kind {
                    "get" => "SecretStoreReadError",
                    "set" => "SecretStorePersistError",
                    _ => "SecretStoreRemoveError",
                },
                name,
            ));
        }
        let result = run();
        if fail.is_some() {
            Err(StoreError::new(
                match kind {
                    "get" => "SecretStoreReadError",
                    "set" => "SecretStorePersistError",
                    _ => "SecretStoreRemoveError",
                },
                name,
            ))
        } else {
            Ok(result)
        }
    }
}
impl SecretStore for Memory {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.operation("get", name, || self.files.borrow().get(name).cloned())
    }
    fn set(&self, name: &str, value: &[u8]) -> Result<(), StoreError> {
        self.operation("set", name, || {
            self.files.borrow_mut().insert(name.into(), value.into());
        })
    }
    fn remove(&self, name: &str) -> Result<(), StoreError> {
        self.operation("remove", name, || {
            self.files.borrow_mut().remove(name);
        })
    }
}
#[test]
fn native_planning_and_reference_only_client_projection() {
    let f = fixture();
    assert_eq!(
        f["knownKinds"],
        serde_json::json!(["one_password"]),
        "update the pinned redaction registry before accepting another kind"
    );
    assert_eq!(f["revision"], "32a0531748c44b25072d7495ce0b9bd7e9450a87");
    for c in f["transactionCases"].as_array().unwrap() {
        let (settings, changes) = tx::plan(&c["current"], &c["next"]);
        assert_eq!(settings, c["settings"], "{} settings", c["label"]);
        assert_eq!(
            serde_json::to_value(changes).unwrap(),
            c["changes"],
            "{} changes",
            c["label"]
        );
        assert_eq!(
            tx::redact(&settings, true),
            c["redactedModern"],
            "{} modern",
            c["label"]
        );
        assert_eq!(
            tx::redact(&settings, false),
            c["redactedLegacy"],
            "{} legacy",
            c["label"]
        );
    }
}
#[test]
fn native_materialization_missing_bom_and_invalid_utf8() {
    for c in fixture()["materializeCases"].as_array().unwrap() {
        let store = Memory::default();
        for (name, value) in c["files"].as_object().unwrap() {
            let name = name
                .strip_prefix("/private-fixture/secrets/")
                .unwrap()
                .strip_suffix(".bin")
                .unwrap();
            store.files.borrow_mut().insert(
                name.into(),
                serde_json::from_value(value["bytes"].clone()).unwrap(),
            );
        }
        assert_eq!(
            tx::materialize(&store, &c["settings"]).unwrap(),
            c["result"],
            "{}",
            c["label"]
        );
    }
}
#[test]
fn reverse_cross_slot_rollback_preserves_primary_error_and_continues() {
    let changes: Vec<tx::Change> =
        serde_json::from_value(fixture()["applyCases"][0]["changes"].clone()).unwrap();
    for failure in [
        None,
        Some(("get", 2, false)),
        Some(("set", 2, false)),
        Some(("set", 1, true)),
        Some(("remove", 1, false)),
    ] {
        let store = Memory::default();
        store
            .files
            .borrow_mut()
            .extend([("first".into(), vec![1]), ("second".into(), vec![2])]);
        *store.fail.borrow_mut() = failure;
        let original = store.files.borrow().clone();
        match tx::apply(&store, &changes) {
            Ok(writes) => {
                assert_eq!(store.files.borrow().get("first"), Some(&vec![10, 20]));
                assert!(tx::rollback(&store, &writes).is_empty());
            }
            Err(error) => {
                assert!(failure.is_some());
                assert_eq!(
                    error.operation,
                    if failure.unwrap().0 == "get" {
                        "read-secret"
                    } else if failure.unwrap().0 == "remove" {
                        "remove-stale-secret"
                    } else {
                        "write-secret"
                    }
                );
            }
        }
        assert_eq!(*store.files.borrow(), original, "{failure:?}");
    }
    let store = Memory::default();
    store.files.borrow_mut().insert("first".into(), vec![1]);
    let writes = tx::apply(&store, &changes).unwrap();
    *store.fail.borrow_mut() = Some(("remove", 2, false));
    let failures = tx::rollback(&store, &writes);
    assert_eq!(failures.len(), 1);
    assert_eq!(store.files.borrow().get("first"), Some(&vec![1]));
}
fn private_directory() -> std::path::PathBuf {
    sandbox::assert_isolated();
    let mut id = [0; 16];
    getrandom::fill(&mut id).unwrap();
    let path = std::env::temp_dir().join(format!(
        "private-secret-store-{:x}",
        u128::from_le_bytes(id)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
#[test]
fn owned_slots_modes_create_race_and_identity_bytes_are_preserved() {
    let root = private_directory();
    let path = root.join("secrets");
    let store = Arc::new(FileSecretStore::open(&path).unwrap());
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    store.set("first", &[1, 2, 3]).unwrap();
    assert_eq!(
        path.join("first.bin")
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(store.get_or_create_random("first", 64).unwrap(), [1, 2, 3]);
    let barrier = Arc::new(Barrier::new(8));
    let tasks = (0..8)
        .map(|_| {
            let (store, barrier) = (store.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                store.get_or_create_random("shared", 32).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let results = tasks
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert!(results.iter().all(|v| v == &results[0]));
    assert_eq!(results[0].len(), 32);
    assert!(store.create("first", &[9]).unwrap_err().already_exists);
    assert_eq!(store.get("first").unwrap(), Some(vec![1, 2, 3]));
    store.remove("first").unwrap();
    store.remove("first").unwrap();
    assert_eq!(store.get("first").unwrap(), None);
    assert!(std::fs::read_dir(path).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}
#[test]
fn paths_and_symlinks_are_refused_within_private_fixture() {
    let root = private_directory();
    let secrets = root.join("secrets");
    let store = FileSecretStore::open(&secrets).unwrap();
    let target = root.join("kept");
    std::fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, secrets.join("link.bin")).unwrap();
    assert!(store.get("link").is_err());
    assert!(store.remove("link").is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    for name in ["../kept", "/kept", "a/b", ".", ""] {
        assert!(store.get(name).is_err());
        assert!(store.set(name, b"bad").is_err());
        assert!(store.remove(name).is_err());
    }
    let linked = root.join("linked-directory");
    std::os::unix::fs::symlink(&secrets, &linked).unwrap();
    assert!(FileSecretStore::open(&linked).is_err());
    assert!(FileSecretStore::open(std::path::Path::new("/")).is_err());
}

#[test]
fn actual_native_apply_failures_and_rollback_outcomes() {
    for case in fixture()["applyCases"].as_array().unwrap() {
        let store = Memory::default();
        for (path, value) in case["input"]["files"].as_object().unwrap() {
            store.files.borrow_mut().insert(
                path.strip_prefix("/private-fixture/secrets/")
                    .unwrap()
                    .strip_suffix(".bin")
                    .unwrap()
                    .into(),
                serde_json::from_value(value["bytes"].clone()).unwrap(),
            );
        }
        let label = case["label"].as_str().unwrap();
        *store.fail.borrow_mut() = match label {
            "read-failure" => Some(("get", 2, false)),
            "write-failure" => Some(("set", 2, false)),
            "after-mutation" => Some(("set", 1, true)),
            "second-remove-failure" => Some(("remove", 1, false)),
            "rollback-failure" => Some(("set", 1, true)),
            _ => None,
        };
        if label == "rollback-failure" {
            store
                .additional_failures
                .borrow_mut()
                .push(("set", 2, false));
        }
        let changes: Vec<tx::Change> = serde_json::from_value(case["changes"].clone()).unwrap();
        match tx::apply(&store, &changes) {
            Ok(writes) => {
                assert!(case["result"].get("error").is_none(), "{label}");
                if label == "rollback" {
                    assert!(tx::rollback(&store, &writes).is_empty());
                }
            }
            Err(error) => {
                let expected = &case["result"]["error"];
                assert_eq!(error.operation, expected["operation"], "{label}");
                assert_eq!(error.cause.tag, expected["causeTag"], "{label}");
                assert_eq!(
                    error.provider_instance_id.as_deref(),
                    expected["providerInstanceId"].as_str(),
                    "{label}"
                );
                assert_eq!(
                    error.environment_variable.as_deref(),
                    expected["environmentVariable"].as_str(),
                    "{label}"
                );
                assert_eq!(
                    error.rollback_errors.len(),
                    usize::from(label == "rollback-failure")
                );
            }
        }
        let expected = case["state"]["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, value)| {
                (
                    path.strip_prefix("/private-fixture/secrets/")
                        .unwrap()
                        .strip_suffix(".bin")
                        .unwrap()
                        .to_owned(),
                    serde_json::from_value::<Vec<u8>>(value["bytes"].clone()).unwrap(),
                )
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(*store.files.borrow(), expected, "{label}");
    }
}
#[test]
fn actual_native_materialization_errors_keep_slot_metadata() {
    for case in fixture()["materializeErrors"].as_array().unwrap() {
        let store = Memory::default();
        *store.fail.borrow_mut() = Some(("get", 1, false));
        let error = tx::materialize(&store, &case["settings"]).unwrap_err();
        let expected = &case["result"]["error"];
        assert_eq!(error.operation, expected["operation"]);
        assert_eq!(error.cause.tag, expected["causeTag"]);
        assert_eq!(
            error.provider_instance_id.as_deref(),
            expected["providerInstanceId"].as_str()
        );
        assert_eq!(
            error.environment_variable.as_deref(),
            expected["environmentVariable"].as_str()
        );
    }
}
#[test]
fn physical_cross_slot_transaction_and_rollback_leave_references_only() {
    let root = private_directory();
    let store = FileSecretStore::open(&root.join("secrets")).unwrap();
    let current = serde_json::json!({"providerInstances":{},"sourceControlHosts":{},"usageLimitSources":{"synthetic":{"managementKey":"first-private-value"}},"easDeviceHosts":[],"simlockDeviceHosts":[],"secretProviders":{}});
    let (settings, changes) = tx::plan(&current, &current);
    let writes = tx::apply(&store, &changes).unwrap();
    assert!(!settings.to_string().contains("first-private-value"));
    assert!(!tx::redact(&settings, true)
        .to_string()
        .contains("first-private-value"));
    assert_eq!(tx::materialize(&store, &settings).unwrap(), current);
    assert!(tx::rollback(&store, &writes).is_empty());
    assert_eq!(store.get(&changes[0].secret_name).unwrap(), None);
    std::fs::create_dir(root.join("secrets/blocked.bin")).unwrap();
    assert!(store.set("blocked", b"private").is_err());
    assert!(store.get("blocked").is_err());
    assert!(store.remove("blocked").is_err());
    assert!(std::fs::read_dir(root.join("secrets")).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}

#[test]
fn temporary_collision_and_fifo_never_remove_or_read_unowned_entries() {
    let root = private_directory();
    let path = root.join("secrets");
    let store = FileSecretStore::open(&path).unwrap();
    let temporary = path.join("collision.bin.fixed-fixture.tmp");
    std::fs::write(&temporary, b"owned-by-another-attempt").unwrap();
    assert!(
        store
            .create_with_fixture_nonce("collision", b"replacement")
            .unwrap_err()
            .already_exists
    );
    assert_eq!(
        std::fs::read(&temporary).unwrap(),
        b"owned-by-another-attempt"
    );
    assert_eq!(store.get("collision").unwrap(), None);
    use std::os::unix::ffi::OsStrExt;
    let fifo = path.join("fifo.bin");
    let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(store.get("fifo").is_err());
    assert!(store.remove("fifo").is_err());
    assert!(fifo.symlink_metadata().unwrap().file_type().is_fifo());
}
