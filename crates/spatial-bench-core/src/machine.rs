//! Machine fingerprint and continuity hash — see design §9.
//!
//! The hash answers exactly one question: *is this the same machine, such that
//! numbers are comparable over time?* Hardware only. Kernel, OS and toolchain
//! change constantly and live in run context, where they can be faceted without
//! severing benchmark continuity.
//!
//! The hash's shape and inputs are **permanent in practice**: once a dataset
//! holds rows, any change here — width, tier split, component set, canonical
//! rendering — severs every historical row's continuity, which is why the
//! shape was fixed before any did. A change to either is a new hash generation
//! and belongs behind the fingerprint file's `schema` field.
//!
//! The hash is **two-part**: `<10 unprivileged chars>-<5 privileged chars or
//! UNKNOWN>`. The prefix is the machine's identity, recomputable by any run;
//! the suffix is what only a fingerprint capture verified, or the literal
//! `UNKNOWN`. See [`Machine::hash`].

use serde::{Deserialize, Serialize};

/// The components that feed the hash, in canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    CpuModel,
    CpuBaseMhz,
    MemSpeedMts,
    MemTimings,
    Board,
    Chipset,
}

pub const HASH_COMPONENTS: &[Component] = &[
    Component::CpuModel,
    Component::CpuBaseMhz,
    Component::MemSpeedMts,
    Component::MemTimings,
    Component::Board,
    Component::Chipset,
];

/// The hash's shape: `<10 unprivileged chars>-<5 privileged chars or UNKNOWN>`.
/// Permanent in practice — see the module docs.
pub const HASH_PREFIX_CHARS: usize = 10;
pub const HASH_SUFFIX_CHARS: usize = 5;

/// The privileged half's sentinel: memory was never verified. Uppercase so it
/// cannot be mistaken for base32 output.
pub const UNKNOWN_SUFFIX: &str = "UNKNOWN";

impl Component {
    /// Whether reading this component needs root on Linux.
    pub fn needs_root(self) -> bool {
        matches!(self, Component::MemSpeedMts | Component::MemTimings)
    }

    /// Where the value comes from, for `bench machine --explain`.
    pub fn source(self) -> &'static str {
        match self {
            Component::CpuModel => "/proc/cpuinfo: model name",
            Component::CpuBaseMhz => {
                "/sys/devices/system/cpu/cpu0/cpufreq/base_frequency, else DMI"
            }
            Component::MemSpeedMts => "dmidecode -t 17 (root)",
            Component::MemTimings => "dmidecode -t 17 (root), else decode-dimms",
            Component::Board => "/sys/devices/virtual/dmi/id/board_{vendor,name}",
            Component::Chipset => "lspci",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Board {
    pub vendor: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub total_bytes: Option<u64>,
    pub speed_mts: Option<u32>,
    /// e.g. "30-36-36-96"
    pub timings: Option<String>,
    /// DIMM part numbers, sorted and deduplicated. Informational: not a hash
    /// component, since the same memory kit is what the timings describe.
    #[serde(default)]
    pub parts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Machine {
    /// Every component the hash *would* cover.
    pub hash_components: Vec<Component>,
    /// Those actually readable on this host.
    pub hash_observed: Vec<Component>,
    /// True when `hash_observed` is a strict subset of `hash_components`.
    pub degraded: bool,

    pub cpu_model: Option<String>,
    pub cpu_base_mhz: Option<u32>,
    pub cores_physical: Option<u32>,
    pub threads_online: Option<u32>,
    pub mem: Memory,
    pub board: Board,
    pub chipset: Option<String>,
}

impl Machine {
    /// A machine with nothing observed. Used by tests and as the starting point
    /// for a probe.
    pub fn unknown() -> Self {
        Self {
            hash_components: HASH_COMPONENTS.to_vec(),
            hash_observed: Vec::new(),
            degraded: true,
            cpu_model: None,
            cpu_base_mhz: None,
            cores_physical: None,
            threads_online: None,
            mem: Memory {
                total_bytes: None,
                speed_mts: None,
                timings: None,
                parts: Vec::new(),
            },
            board: Board {
                vendor: None,
                name: None,
            },
            chipset: None,
        }
    }

    /// Probe the host without privileges. Never fails: anything unreadable
    /// becomes `None` and drops out of `hash_observed`, leaving a degraded
    /// hash — which is honest, because such a run verified less.
    ///
    /// The root-only components (memory speed, timings, part numbers) are NOT
    /// read here; see [`Machine::add_privileged`] and the fingerprint file,
    /// which is how an unprivileged run gets a non-degraded machine hash.
    pub fn probe() -> Self {
        let mut m = Self::unknown();
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            m.cpu_model = parse_cpu_model(&cpuinfo);
            m.cores_physical = parse_cpu_cores(&cpuinfo);
            m.threads_online = Some(count_processors(&cpuinfo));
        }
        m.cpu_base_mhz = read_base_mhz();
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            m.mem.total_bytes = parse_meminfo_total(&meminfo);
        }
        m.board.vendor = read_trimmed("/sys/class/dmi/id/board_vendor");
        m.board.name = read_trimmed("/sys/class/dmi/id/board_name");
        m.chipset = probe_chipset();
        m.refresh_observed();
        m
    }

    /// Read the root-only components: memory speed, timings, part numbers.
    ///
    /// Best effort per field — `decode-dimms` being absent leaves timings
    /// `None`, which degrades the hash rather than failing the capture — but
    /// an unreadable *core* (`dmidecode` missing, or no root) is an error,
    /// because a capture that silently recorded no memory block would look
    /// complete to anyone who did not read the `MISS` lines.
    pub fn add_privileged(&mut self) -> Result<(), PrivilegedError> {
        if !is_root() {
            return Err(PrivilegedError::NotRoot);
        }
        let raw = run_capture(
            // absolute paths first. Under `sudo`, a bare name resolves
            // through root's PATH, and a planted `dmidecode` earlier in it
            // would execute as root. The bare-name fallback keeps unusual
            // layouts working, at the usual PATH trust level.
            &["/usr/sbin/dmidecode", "/sbin/dmidecode", "dmidecode"],
            &["-t", "17"],
        )
        .ok_or(PrivilegedError::DmidecodeMissing)?;
        let (speed, parts) = parse_dmidecode(&raw);
        self.mem.speed_mts = speed;
        self.mem.parts = parts;

        // Timings live in the SPD, not DMI, so dmidecode cannot supply them.
        // decode-dimms needs the eeprom modules and root; absent is normal.
        if let Some(raw) = run_capture(&["/usr/sbin/decode-dimms", "decode-dimms"], &[]) {
            self.mem.timings = parse_dimm_timings(&raw);
        }
        self.refresh_observed();
        Ok(())
    }

    /// Recompute `hash_observed` and `degraded` from which fields are populated.
    pub fn refresh_observed(&mut self) {
        self.hash_observed = HASH_COMPONENTS
            .iter()
            .copied()
            .filter(|c| self.has(*c))
            .collect();
        self.degraded = self.hash_observed.len() != self.hash_components.len();
    }

    fn has(&self, component: Component) -> bool {
        match component {
            Component::CpuModel => self.cpu_model.is_some(),
            Component::CpuBaseMhz => self.cpu_base_mhz.is_some(),
            Component::MemSpeedMts => self.mem.speed_mts.is_some(),
            Component::MemTimings => self.mem.timings.is_some(),
            Component::Board => self.board.vendor.is_some() || self.board.name.is_some(),
            Component::Chipset => self.chipset.is_some(),
        }
    }

    fn value_of(&self, component: Component) -> String {
        match component {
            Component::CpuModel => self.cpu_model.clone().unwrap_or_default(),
            Component::CpuBaseMhz => self.cpu_base_mhz.map(|v| v.to_string()).unwrap_or_default(),
            Component::MemSpeedMts => self
                .mem
                .speed_mts
                .map(|v| v.to_string())
                .unwrap_or_default(),
            Component::MemTimings => self.mem.timings.clone().unwrap_or_default(),
            Component::Board => format!(
                "{}/{}",
                self.board.vendor.clone().unwrap_or_default(),
                self.board.name.clone().unwrap_or_default()
            ),
            Component::Chipset => self.chipset.clone().unwrap_or_default(),
        }
    }

    /// The continuity hash, in two privilege tiers:
    /// `<unprivileged>-<privileged>`, e.g. `e66e3ymtef-k3vxq`, or
    /// `e66e3ymtef-UNKNOWN` when memory was never verified.
    ///
    /// The split exists because the two halves change for different reasons.
    /// The prefix covers what any run on this box can read — it is the
    /// machine's identity, stable whether or not a fingerprint exists, and
    /// recomputable without root. The suffix covers what only a fingerprint
    /// capture could read; it is the literal `UNKNOWN` when nothing root-only
    /// was determined, so an unverified run is visibly unverified instead of
    /// wearing a hash it cannot explain.
    ///
    /// The **observed-component set is part of each half's input** — the two
    /// halves are joined with `-` but never merged into one input, so a
    /// readability change moves only the half it belongs to. Installing
    /// decode-dimms and re-capturing changes the suffix; the prefix proves it
    /// is still the same box. And a run that verified less still never
    /// collides with one that verified more: the suffix differs.
    ///
    /// The full string is the join key. Charts join on all of it, never the
    /// prefix alone — joining on the prefix would merge unverified runs into
    /// verified history. Widths are permanent in practice; see the module docs.
    pub fn hash(&self) -> String {
        format!("{}-{}", self.unprivileged_hash(), self.privileged_hash())
    }

    /// The unprivileged half: what any run on this box can recompute.
    pub fn unprivileged_hash(&self) -> String {
        self.hash_tier(false, HASH_PREFIX_CHARS)
    }

    /// The privileged half, or the `UNKNOWN` sentinel when nothing root-only
    /// was read — no fingerprint, or a capture whose privileged block is empty.
    /// Both mean the same thing: memory was never verified.
    pub fn privileged_hash(&self) -> String {
        let observed: Vec<Component> = self
            .hash_observed
            .iter()
            .copied()
            .filter(|c| c.needs_root())
            .collect();
        if observed.is_empty() {
            return UNKNOWN_SUFFIX.to_owned();
        }
        self.hash_tier_observed(&observed, HASH_SUFFIX_CHARS)
    }

    fn hash_tier(&self, root: bool, chars: usize) -> String {
        let observed: Vec<Component> = self
            .hash_observed
            .iter()
            .copied()
            .filter(|c| c.needs_root() == root)
            .collect();
        self.hash_tier_observed(&observed, chars)
    }

    fn hash_tier_observed(&self, observed: &[Component], chars: usize) -> String {
        // The observed SET is hashed first, before any value. Two probes that
        // read different components did not verify the same things, so they
        // must not collide and imply a continuity nobody checked.
        let mut canonical = String::new();
        for component in observed {
            canonical.push_str(&format!("{component:?};"));
        }
        canonical.push('|');
        for component in observed {
            canonical.push_str(&format!("{component:?}={};", self.value_of(*component)));
        }
        base32_crockford(blake3::hash(canonical.as_bytes()).as_bytes(), chars)
    }

    /// True when some component the hash covers could not be read here.
    pub fn is_degraded(&self) -> bool {
        self.hash_observed.len() != self.hash_components.len()
    }

    /// Human-readable account of what was and was not readable, and why.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("machine hash: {}\n", self.hash()));
        if self.is_degraded() {
            out.push_str(
                "  DEGRADED: some components were unreadable; this hash will not match one\n                 \x20 taken on the same machine with them present, by design.\n",
            );
        }
        for component in &self.hash_components {
            let (mark, value) = if self.has(*component) {
                ("ok  ", self.value_of(*component))
            } else if component.needs_root() {
                ("MISS", "unreadable without root".to_owned())
            } else {
                ("MISS", "unreadable".to_owned())
            };
            out.push_str(&format!(
                "  [{mark}] {component:?}: {value}\n         from {}\n",
                component.source()
            ));
        }
        out
    }
}

/// Hash arbitrary bytes with the same function the machine hash uses, so a run
/// id and a machine hash cannot disagree about what hashing means here.
pub fn hash_bytes(bytes: &[u8]) -> blake3::Hash {
    blake3::hash(bytes)
}

/// True when running as root, read from /proc rather than a libc call so the
/// crate stays dependency-light for what is one boolean.
pub fn is_root() -> bool {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .is_some_and(|uid| uid == "0")
}

/// Run `candidates[0]` (or the first that exists) with `args`, returning stdout.
/// `None` when none of the candidates could be spawned at all.
fn run_capture(candidates: &[&str], args: &[&str]) -> Option<String> {
    for name in candidates {
        if let Ok(out) = std::process::Command::new(name).args(args).output() {
            if out.status.success() {
                return Some(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    None
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Base clock, unprivileged. Intel's cpufreq exposes `base_frequency` (kHz);
/// amd_pstate exposes `nominal_freq` (MHz) — often empty, in which case the
/// component honestly stays unreadable and the hash degrades rather than
/// recording a boost clock under a base-clock name.
fn read_base_mhz() -> Option<u32> {
    if let Ok(raw) = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/base_frequency")
    {
        if let Ok(khz) = raw.trim().parse::<u32>() {
            return Some(khz / 1000);
        }
    }
    if let Ok(raw) = std::fs::read_to_string("/sys/devices/system/cpu/amd_pstate/nominal_freq") {
        return raw.trim().parse::<u32>().ok();
    }
    None
}

/// The chipset, from the PCI host bridge description. lspci's wording is what
/// it is; the point is a stable string that changes with the silicon.
fn probe_chipset() -> Option<String> {
    // absolute paths first — see add_privileged.
    run_capture(&["/usr/sbin/lspci", "/usr/bin/lspci", "lspci"], &[])
        .and_then(|out| parse_lspci_chipset(&out))
}

fn parse_cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo
        .lines()
        .find_map(|l| l.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, v)| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn parse_cpu_cores(cpuinfo: &str) -> Option<u32> {
    cpuinfo
        .lines()
        .find_map(|l| l.strip_prefix("cpu cores"))
        .and_then(|rest| rest.split_once(':'))
        .and_then(|(_, v)| v.trim().parse().ok())
}

fn count_processors(cpuinfo: &str) -> u32 {
    cpuinfo
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count() as u32
}

/// `/proc/meminfo`'s MemTotal is usable memory: the kernel's reservations
/// shift it by a few megabytes between boots on identical hardware, so the
/// raw figure is round downd to 64 MiB before anything stores or compares it.
/// A real RAM upgrade moves the value by a full 64 MiB step; a reboot must not
/// move it at all. (The gate once claimed exact comparison could not
/// false-positive; a 3.8 MB post-reboot drift on a Ryzen 5 8500GE said
/// otherwise and refused every run on that machine.)
pub const MEM_TOTAL_ROUNDING: u64 = 64 << 20; // 64 MiB

fn parse_meminfo_total(meminfo: &str) -> Option<u64> {
    const ROUND: u64 = MEM_TOTAL_ROUNDING;
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| kb * 1024 / ROUND * ROUND)
}

/// Memory speed and part numbers from `dmidecode -t 17`.
///
/// `Configured Memory Speed` is preferred over the DIMM's rated `Speed`: the
/// configured figure is what the machine actually runs at, which is what a
/// continuity hash cares about.
fn parse_dmidecode(out: &str) -> (Option<u32>, Vec<String>) {
    let mut speed = None;
    let mut rated = None;
    let mut parts = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.trim().strip_prefix("Configured Memory Speed:") {
            speed = mts(rest);
        } else if let Some(rest) = line.trim().strip_prefix("Speed:") {
            rated = rated.or_else(|| mts(rest));
        } else if let Some(rest) = line.trim().strip_prefix("Part Number:") {
            let part = rest.trim();
            if !part.is_empty() && part != "Not Specified" {
                parts.push(part.to_owned());
            }
        }
    }
    parts.sort();
    parts.dedup();
    (speed.or(rated), parts)
}

fn mts(rest: &str) -> Option<u32> {
    rest.split_whitespace().next()?.parse().ok()
}

/// Timings from `decode-dimms`, e.g. `tCL-tRCD-tRP-tRAS as DDR5-6000: 30-36-36-96`.
/// DMI has no CAS latency; the SPD does, through this tool.
fn parse_dimm_timings(out: &str) -> Option<String> {
    out.lines().filter(|l| l.contains("tRCD")).find_map(|l| {
        let (_, after) = l.rsplit_once(':')?;
        let after = after.trim();
        let groups: Vec<&str> = after.split_whitespace().next()?.split('-').collect();
        (groups.len() == 4 && groups.iter().all(|g| g.chars().all(|c| c.is_ascii_digit())))
            .then(|| groups.join("-"))
    })
}

/// `00:00.0 Host bridge: Advanced Micro Devices, Inc. [AMD] Device 1482 (rev 01)`
/// becomes `Advanced Micro Devices, Inc. [AMD] Device 1482` — the trailing
/// revision and, in `-mm` output, PCI ID are dropped so a BIOS update that
/// renumbers nothing does not break continuity.
fn parse_lspci_chipset(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.contains("Host bridge"))?;
    let desc = line
        .split_once("Host bridge:")
        .or_else(|| line.split_once("\"Host bridge\" "))
        .map(|(_, rest)| rest.trim())?;
    let desc = desc
        .split_once(" (rev")
        .map(|(head, _)| head.trim())
        .unwrap_or(desc);
    // Strip a trailing " [1234:5678]" only: an embedded "[AMD]" stays.
    let desc = desc
        .strip_suffix(']')
        .and_then(|head| head.rsplit_once(" ["))
        .filter(|(_, id)| {
            matches!(id.split_once(':'), Some((l, r))
                if !l.is_empty() && !r.is_empty()
                    && l.chars().all(|c| c.is_ascii_hexdigit())
                    && r.chars().all(|c| c.is_ascii_hexdigit()))
        })
        .map(|(head, _)| head)
        .unwrap_or(desc);
    (!desc.is_empty()).then(|| desc.to_owned())
}

/// Why the root-only components could not be read.
#[derive(Debug, PartialEq)]
pub enum PrivilegedError {
    /// Everything needing root was skipped: not running as root.
    NotRoot,
    /// dmidecode is not installed at any usual location.
    DmidecodeMissing,
}

/// Crockford-style base32, lowercase. `chars` characters = `5 × chars` bits,
/// taken from the head of the digest.
fn base32_crockford(bytes: &[u8], chars: usize) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    // `chars` characters need `5 × chars` bits: 10 chars = 50 bits = 7 bytes.
    let mut bits: u64 = 0;
    for byte in bytes.iter().take((chars * 5).div_ceil(8)) {
        bits = (bits << 8) | u64::from(*byte);
    }
    (0..chars)
        .rev()
        .map(|i| ALPHABET[((bits >> (i * 5)) & 0x1f) as usize] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Machine {
        let mut m = Machine::unknown();
        m.cpu_model = Some("AMD Ryzen 9 9950X 16-Core Processor".into());
        m.cpu_base_mhz = Some(4300);
        m.mem.speed_mts = Some(6000);
        m.mem.timings = Some("30-36-36-96".into());
        m.board = Board {
            vendor: Some("ASUSTeK".into()),
            name: Some("ROG STRIX X870E-E".into()),
        };
        m.chipset = Some("AMD X870E".into());
        m.refresh_observed();
        m
    }

    #[test]
    fn hash_is_two_tiers_prefix_dash_suffix() {
        let h = full().hash();
        let (prefix, suffix) = h.split_once('-').expect("two-part shape");
        assert_eq!(prefix.len(), HASH_PREFIX_CHARS);
        assert_eq!(suffix.len(), HASH_SUFFIX_CHARS);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{h}"
        );
    }

    /// The point of the two-part shape: what a run can verify differs between
    /// an unprivileged probe and a capture, but the machine does not. The
    /// prefix is identical; only the suffix moves, and an unverified probe
    /// says UNKNOWN outright.
    #[test]
    fn the_prefix_survives_a_change_in_what_was_verified() {
        let capture = full();
        let mut probe = full();
        probe.mem.speed_mts = None;
        probe.mem.timings = None;
        probe.refresh_observed();

        assert_eq!(capture.unprivileged_hash(), probe.unprivileged_hash());
        assert_eq!(probe.privileged_hash(), UNKNOWN_SUFFIX);
        assert_ne!(capture.privileged_hash(), probe.privileged_hash());
        assert_ne!(capture.hash(), probe.hash());
    }

    #[test]
    fn same_hardware_hashes_the_same() {
        assert_eq!(full().hash(), full().hash());
    }

    #[test]
    fn changing_any_component_changes_the_hash() {
        let base = full().hash();
        let mut cpu = full();
        cpu.cpu_model = Some("AMD Ryzen 9 7950X 16-Core Processor".into());
        let mut mem = full();
        mem.mem.timings = Some("32-38-38-96".into());
        let mut board = full();
        board.board.name = Some("ROG STRIX B650E-F".into());
        for (label, other) in [
            ("cpu", cpu.hash()),
            ("mem timings", mem.hash()),
            ("board", board.hash()),
        ] {
            assert_ne!(base, other, "{label} change should break continuity");
        }
    }

    /// The point of hashing the observed set: a run without root did not verify
    /// the same things as one with it, so it must not silently share a history.
    #[test]
    fn a_degraded_probe_does_not_collide_with_a_full_one() {
        let mut degraded = full();
        degraded.mem.speed_mts = None;
        degraded.mem.timings = None;
        degraded.refresh_observed();

        assert!(degraded.is_degraded());
        assert!(!full().is_degraded());
        assert_ne!(degraded.hash(), full().hash());
    }

    /// Two machines differing only in a component neither could read still hash
    /// differently if anything they *did* read differs.
    #[test]
    fn degraded_hashes_still_distinguish_what_was_seen() {
        let mut a = full();
        a.mem.timings = None;
        a.refresh_observed();
        let mut b = a.clone();
        b.cpu_base_mhz = Some(3800);
        b.refresh_observed();
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn explain_names_every_component_and_flags_degradation() {
        let text = full().explain();
        for component in HASH_COMPONENTS {
            assert!(
                text.contains(&format!("{component:?}")),
                "missing {component:?}"
            );
        }
        assert!(!text.contains("DEGRADED"));

        let mut partial = full();
        partial.mem.timings = None;
        partial.refresh_observed();
        let text = partial.explain();
        assert!(text.contains("DEGRADED"));
        assert!(text.contains("unreadable without root"));
    }

    // ---- parsers ----------------------------------------------------------

    const CPUINFO: &str = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 9 9950X 16-Core Processor\ncpu cores\t: 16\nprocessor\t: 1\n";

    #[test]
    fn cpuinfo_parses() {
        assert_eq!(
            parse_cpu_model(CPUINFO).as_deref(),
            Some("AMD Ryzen 9 9950X 16-Core Processor")
        );
        assert_eq!(parse_cpu_cores(CPUINFO), Some(16));
        assert_eq!(count_processors(CPUINFO), 2);
    }

    #[test]
    fn meminfo_parses_kib_to_bytes() {
        // Round downd to 64 MiB: 32732160 kB raw = 33487323136 B exact, which
        // rounds down to 496 steps of 64 MiB. A value already on a 64 MiB
        // multiple is exact.
        assert_eq!(
            parse_meminfo_total("MemTotal:       32732160 kB\n"),
            Some(499 * (64 << 20))
        );
        assert_eq!(
            parse_meminfo_total("MemTotal:       32768000 kB\n"),
            Some(500 * (64 << 20))
        );
    }

    #[test]
    fn dmidecode_prefers_configured_speed_and_skips_blank_parts() {
        let raw = "Memory Device\n\tSpeed: 5600 MT/s\n\tPart Number: F5-6000J3036G32GX2-TZ5NR\n\tConfigured Memory Speed: 6000 MT/s\n\tPart Number: Not Specified\n";
        let (speed, parts) = parse_dmidecode(raw);
        assert_eq!(speed, Some(6000));
        assert_eq!(parts, vec!["F5-6000J3036G32GX2-TZ5NR".to_owned()]);
    }

    #[test]
    fn dmidecode_falls_back_to_rated_speed() {
        let (speed, _) = parse_dmidecode("\tSpeed: 5600 MT/s\n");
        assert_eq!(speed, Some(5600));
    }

    #[test]
    fn dimm_timings_parse_four_hyphenated_numbers() {
        let raw = "---=== Timings at Standard Speed ===---\ntCL-tRCD-tRP-tRAS as DDR5-6000: 30-36-36-96\n";
        assert_eq!(parse_dimm_timings(raw).as_deref(), Some("30-36-36-96"));
        // Three groups, or none, is not a timing set.
        assert_eq!(parse_dimm_timings("tCL-tRCD-tRP-tRAS: 30-36-36\n"), None);
        assert_eq!(parse_dimm_timings("no numbers here\n"), None);
    }

    #[test]
    fn lspci_takes_the_host_bridge_description() {
        let raw = "00:00.0 Host bridge: Advanced Micro Devices, Inc. [AMD] Device 1482 (rev 01)\n01:00.0 VGA compatible controller [0300]: AMD Device 1482\n";
        assert_eq!(
            parse_lspci_chipset(raw).as_deref(),
            Some("Advanced Micro Devices, Inc. [AMD] Device 1482")
        );
        // A trailing PCI ID, should lspci ever emit one, is stripped while the
        // embedded vendor bracket is not.
        let id =
            "00:00.0 Host bridge: Advanced Micro Devices, Inc. [AMD] Device 1482 [1022:1482]\n";
        assert_eq!(
            parse_lspci_chipset(id).as_deref(),
            Some("Advanced Micro Devices, Inc. [AMD] Device 1482")
        );
        assert_eq!(parse_lspci_chipset("no bridge here"), None);
    }
}
