//! Standalone Linux x64 fixture: exact production module, real libc ABI calls.
//! No Cargo, fake clock, Tokio adapter or production Control integration.
extern crate self as libc;
#[cfg(not(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
)))]
compile_error!("This standalone libc ABI fixture is admitted only on Linux x64");
#[repr(C)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}
pub use Timespec as timespec;
pub const CLOCK_MONOTONIC: i32 = 1;
unsafe extern "C" {
    pub fn clock_gettime(clock: i32, sample: *mut Timespec) -> i32;
}
const _: () = assert!(std::mem::size_of::<Timespec>() == 16);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Protocol,
    Deadline,
    InvalidInstallation,
}
#[path = "../../src/native_page_worker/source_deadline.rs"]
mod source_deadline;
use source_deadline::FrozenDeadline;
use std::{
    collections::HashMap,
    io::{BufRead, Write},
    time::{Duration, Instant},
};
fn difference(left: Instant, right: Instant) -> i128 {
    if left >= right {
        i128::try_from(left.duration_since(right).as_nanos()).unwrap()
    } else {
        -i128::try_from(right.duration_since(left).as_nanos()).unwrap()
    }
}
fn selftest() -> usize {
    let mut count = 0;
    for value in [
        "",
        "-0",
        "+0",
        "00",
        "01",
        "-01",
        " 1",
        "1 ",
        "1e3",
        "1.0",
        "NaN",
        "∞",
        "١",
        "170141183460469231731687303715884105728",
        "-170141183460469231731687303715884105729",
    ] {
        assert!(matches!(
            FrozenDeadline::capture(value, "0"),
            Err(Fault::Protocol)
        ));
        count += 1;
    }
    assert!(matches!(
        FrozenDeadline::capture("1", "0"),
        Err(Fault::Protocol)
    ));
    count += 1;
    assert!(matches!(
        FrozenDeadline::capture(
            "170141183460469231731687303715884105727",
            "170141183460469231731687303715884105727"
        ),
        Err(Fault::Deadline)
    ));
    count += 1;
    assert!(matches!(
        FrozenDeadline::capture("-170141183460469231731687303715884105728", "0"),
        Err(Fault::Protocol)
    ));
    count += 1;
    let past = FrozenDeadline::capture("-1", "0").unwrap();
    assert!(past.expired());
    count += 1;
    count
}
fn main() {
    let mut stored = HashMap::new();
    let mut effects = 0u32;
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        assert!(line.len() <= 512);
        let words: Vec<_> = line.split_ascii_whitespace().collect();
        match words.as_slice() {
            ["selftest"] => println!("SELFTEST {}", selftest()),
            ["capture", id, lower, upper] => {
                let id = id.parse::<u32>().unwrap();
                assert!(stored.len() < 64 && !stored.contains_key(&id));
                match FrozenDeadline::capture(lower, upper) {
                    Ok(frozen) => {
                        let (before, instant, after) = frozen.calibration_sample();
                        let (lo, hi) = frozen.source_bounds();
                        assert!(before <= after);
                        let actual = difference(frozen.std_instant(), instant);
                        assert_eq!(actual, lo - after);
                        println!(
                            "CAPTURE {id} {before} {after} {lo} {hi} {actual} {}",
                            u8::from(frozen.expired())
                        );
                        stored.insert(id, (frozen, frozen.std_instant()));
                    }
                    Err(error) => println!("REJECT {id} {error:?}"),
                }
            }
            ["entry", id] => {
                let id = id.parse::<u32>().unwrap();
                let (frozen, original) = stored.get(&id).unwrap();
                assert_eq!(frozen.std_instant(), *original);
                let expired = frozen.expired();
                if !expired {
                    effects += 1;
                }
                println!("ENTRY {id} {} {effects}", u8::from(expired));
            }
            ["wait", milliseconds] => {
                let milliseconds = milliseconds.parse::<u64>().unwrap();
                assert!(milliseconds <= 200);
                std::thread::sleep(Duration::from_millis(milliseconds));
                println!("WAIT");
            }
            ["quit"] => break,
            _ => panic!("Unadmitted fixed fixture command"),
        }
        std::io::stdout().flush().unwrap();
    }
}
