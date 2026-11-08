//! Frozen admission-only conversion from the original Node monotonic deadline.
//! Used by the private owner-library caller/control path; no public MCP installation.
use super::Fault;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub struct FrozenDeadline {
    instant: Instant,
    source_lower_ns: i128,
    source_upper_ns: i128,
    sample_instant: Instant,
    sample_before_ns: i128,
    sample_after_ns: i128,
}
fn canonical_signed(value: &str) -> Result<i128, Fault> {
    if value.is_empty() || value.len() > 40 {
        return Err(Fault::Protocol);
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || value == "-0"
    {
        return Err(Fault::Protocol);
    }
    value.parse().map_err(|_| Fault::Protocol)
}
#[cfg(target_os = "linux")]
fn monotonic_ns() -> Result<i128, Fault> {
    let mut sample = std::mem::MaybeUninit::<libc::timespec>::uninit();
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, sample.as_mut_ptr()) } != 0 {
        return Err(Fault::InvalidInstallation);
    }
    let sample = unsafe { sample.assume_init() };
    if sample.tv_sec < 0 || !(0..1_000_000_000).contains(&sample.tv_nsec) {
        return Err(Fault::InvalidInstallation);
    }
    i128::from(sample.tv_sec)
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(i128::from(sample.tv_nsec)))
        .ok_or(Fault::InvalidInstallation)
}
#[cfg(not(target_os = "linux"))]
fn monotonic_ns() -> Result<i128, Fault> {
    Err(Fault::InvalidInstallation)
}
fn duration_ns(value: u128) -> Result<Duration, Fault> {
    let seconds = u64::try_from(value / 1_000_000_000).map_err(|_| Fault::Deadline)?;
    let nanoseconds = u32::try_from(value % 1_000_000_000).map_err(|_| Fault::Deadline)?;
    Ok(Duration::new(seconds, nanoseconds))
}
fn shift(base: Instant, delta_ns: i128) -> Result<Instant, Fault> {
    let duration = duration_ns(delta_ns.unsigned_abs())?;
    if delta_ns >= 0 {
        base.checked_add(duration)
    } else {
        base.checked_sub(duration)
    }
    .ok_or(Fault::Deadline)
}
impl FrozenDeadline {
    pub(super) fn capture(lower_ns: &str, upper_ns: &str) -> Result<Self, Fault> {
        let lower = canonical_signed(lower_ns)?;
        let upper = canonical_signed(upper_ns)?;
        if lower > upper {
            return Err(Fault::Protocol);
        }
        let before = monotonic_ns()?;
        let instant = Instant::now();
        let after = monotonic_ns()?;
        Self::from_bracket(lower, upper, before, instant, after)
    }
    fn from_bracket(
        lower: i128,
        upper: i128,
        before: i128,
        instant: Instant,
        after: i128,
    ) -> Result<Self, Fault> {
        if lower > upper || before < 0 || before > after {
            return Err(Fault::Protocol);
        }
        // instant's real coordinate is <= after. Mapping against the upper
        // bracket endpoint can only shorten, never extend, the source lower bound.
        let delta = lower.checked_sub(after).ok_or(Fault::Protocol)?;
        Ok(Self {
            instant: shift(instant, delta)?,
            source_lower_ns: lower,
            source_upper_ns: upper,
            sample_instant: instant,
            sample_before_ns: before,
            sample_after_ns: after,
        })
    }
    /// Transfer to Tokio with Instant::from_std, never a second now()+remaining.
    /// This deadline applies once at queued callback entry, not to an entered tail.
    pub fn std_instant(&self) -> Instant {
        self.instant
    }
    pub fn expired(&self) -> bool {
        Instant::now() >= self.instant
    }
    pub fn source_bounds(&self) -> (i128, i128) {
        (self.source_lower_ns, self.source_upper_ns)
    }
    pub fn calibration_sample(&self) -> (i128, Instant, i128) {
        (
            self.sample_before_ns,
            self.sample_instant,
            self.sample_after_ns,
        )
    }
}
