//! Machine fingerprint and continuity hash — see design §7.
//!
//! The hash answers exactly one question: *is this the same machine, such that
//! numbers are comparable over time?* Hardware only. Kernel, OS and toolchain
//! change constantly and live in run context, where they can be faceted without
//! severing benchmark continuity.

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
            },
            board: Board {
                vendor: None,
                name: None,
            },
            chipset: None,
        }
    }

    /// Probe the host. Never fails: anything unreadable becomes `None` and drops
    /// out of `hash_observed`.
    pub fn probe() -> Self {
        unimplemented!("needs this host: see Component::source for what it reads")
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

    /// Short continuity hash.
    ///
    /// The **observed-component set is part of the hash input**. That is the
    /// whole point: a run where memory timings were unavailable must not hash
    /// identically to one on the same machine where they were read, because the
    /// two runs did not verify the same things. Silent collision would assert a
    /// continuity nobody checked.
    ///
    /// Returns 6 base32 characters of blake3 over a canonical rendering.
    pub fn hash(&self) -> String {
        let mut canonical = String::new();
        // The observed SET is hashed first, before any value. Two runs on the
        // same box that observed different components did not verify the same
        // things, so they must not collide and imply a continuity nobody
        // checked.
        for component in &self.hash_observed {
            canonical.push_str(&format!("{component:?};"));
        }
        canonical.push('|');
        for component in &self.hash_observed {
            canonical.push_str(&format!("{component:?}={};", self.value_of(*component)));
        }
        base32_6(blake3::hash(canonical.as_bytes()).as_bytes())
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

/// Six characters of Crockford-style base32, lowercase. ~1e9 values: ample for a
/// personal fleet, short enough to sit in a filename.
fn base32_6(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut bits: u64 = 0;
    for byte in bytes.iter().take(4) {
        bits = (bits << 8) | u64::from(*byte);
    }
    (0..6)
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
    fn hash_is_six_lowercase_base32_chars() {
        let h = full().hash();
        assert_eq!(h.len(), 6);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "{h}"
        );
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
}
