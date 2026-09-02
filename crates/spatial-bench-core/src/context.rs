//! Run-level environment context (§10): everything outside the machine's
//! hardware that changed since the last run and that a chart may want to facet
//! by. Deliberately NOT part of point identity — that is what lets one chart
//! overlay the same case across kernels or boot profiles.

use crate::schema::Context;

/// Probe this host. Never fails: anything unreadable stays `None`, because a
/// missing governor line must not stop a run — it is context, not identity.
pub fn probe() -> Context {
    Context {
        kernel: read_trimmed("/proc/sys/kernel/osrelease"),
        os: pretty_os_name(),
        bench_profile: std::env::var("BENCH_PROFILE").ok(),
        governor: read_trimmed("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        smt: read_trimmed("/sys/devices/system/cpu/smt/active").map(|v| v == "1"),
        boost: read_trimmed("/sys/devices/system/cpu/cpufreq/boost").map(|v| v == "1"),
        isolated_cpus: read_trimmed("/sys/devices/system/cpu/isolated"),
        // The run gate fills this in from the fingerprint it validated
        // against (S5); a probe has no file to name.
        fingerprint: None,
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// `PRETTY_NAME` from os-release, e.g. "Fedora Linux 43 (Workstation Edition)".
fn pretty_os_name() -> Option<String> {
    let raw = std::fs::read_to_string("/etc/os-release").ok()?;
    raw.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_owned())
        .filter(|v| !v.is_empty())
}
