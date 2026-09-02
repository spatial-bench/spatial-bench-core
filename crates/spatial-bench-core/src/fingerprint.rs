//! The machine fingerprint file — see design §9.
//!
//! Two-phase: a privileged capture once per machine
//! (`sudo spatial-bench fingerprint --write`), unprivileged validation on every
//! run. Before measuring, the engine re-probes the host's unprivileged
//! components and compares them against the file; a partial match bails,
//! because a stale fingerprint would attribute new hardware's numbers to the
//! old machine's history, which is worse than refusing to run.
//!
//! The `checksum` field is **tamper-evidence, not tamper-resistance**: an
//! HMAC-SHA256 over the canonical file body, keyed by a compile-time constant.
//! It catches an accidental edit or a file copied from another machine and
//! adjusted; it stops nobody who wants to forge a result, and is never
//! described as a signature anywhere.

use crate::machine::{Machine, PrivilegedError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FINGERPRINT_SCHEMA: u32 = 1;

/// Keyed with a constant that ships in an open-source binary, deliberately:
/// a passing check is evidence of an unedited file, not of provenance.
const CHECKSUM_KEY: &[u8] = b"spatial-bench fingerprint checksum v1";

/// Where the fingerprint lives. Machine-scoped rather than per-user or
/// per-checkout: it describes hardware, and one machine with six kiddo
/// worktrees has one fingerprint. The environment variable overrides for
/// containers and CI.
pub fn path() -> PathBuf {
    std::env::var_os("SPATIAL_BENCH_FINGERPRINT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/spatial-bench/fingerprint.toml"))
}

/// The exact command that (re)captures this machine, for error messages: the
/// design requires the missing-file bail to name it.
pub fn capture_command() -> String {
    "sudo spatial-bench fingerprint --write".to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fingerprint {
    pub schema: u32,
    pub taken: String,
    /// The continuity hash of the machine this file describes.
    pub machine: String,
    pub unprivileged: Unprivileged,
    pub privileged: Privileged,
    #[serde(default)]
    pub checksum: String,
}

/// Re-validated on every run, without root.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Unprivileged {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_base_mhz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chipset: Option<String>,
    /// Total usable RAM from /proc/meminfo. Not a hash component — the kit's
    /// speed and timings are — but a hardware fact: a change is a change, and
    /// the run-time footprint check needs it on fingerprinted machines, where
    /// the run's machine comes from this file rather than a fresh probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_total_bytes: Option<u64>,
}

/// Captured once, under root.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Privileged {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_speed_mts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_timings: Option<String>,
    #[serde(default)]
    pub mem_parts: Vec<String>,
}

impl Fingerprint {
    /// Capture this machine. `privileged` reads the root-only block and fails
    /// with [`FingerprintError::NotRoot`] or [`FingerprintError::DmidecodeMissing`]
    /// when it cannot; a missing `decode-dimms` merely leaves timings absent,
    /// which degrades the hash rather than the capture.
    pub fn capture(privileged: bool) -> Result<Self, FingerprintError> {
        let mut machine = Machine::probe();
        if privileged {
            machine.add_privileged().map_err(FingerprintError::from)?;
        }
        Ok(Self {
            schema: FINGERPRINT_SCHEMA,
            taken: crate::schema::utc_timestamp(),
            machine: machine.hash(),
            unprivileged: Unprivileged {
                cpu_model: machine.cpu_model.clone(),
                cpu_base_mhz: machine.cpu_base_mhz,
                board_vendor: machine.board.vendor.clone(),
                board_name: machine.board.name.clone(),
                chipset: machine.chipset.clone(),
                mem_total_bytes: machine.mem.total_bytes,
            },
            privileged: Privileged {
                mem_speed_mts: machine.mem.speed_mts,
                mem_timings: machine.mem.timings.clone(),
                mem_parts: machine.mem.parts.clone(),
            },
            checksum: String::new(),
        })
    }

    /// The machine this file describes, rebuilt from its components. The
    /// rebuilt hash must equal the recorded one: a file whose components
    /// disagree with its own `machine` field was edited by hand.
    pub fn machine(&self) -> Result<Machine, FingerprintError> {
        let mut m = Machine::unknown();
        m.cpu_model = self.unprivileged.cpu_model.clone();
        m.cpu_base_mhz = self.unprivileged.cpu_base_mhz;
        m.board = crate::machine::Board {
            vendor: self.unprivileged.board_vendor.clone(),
            name: self.unprivileged.board_name.clone(),
        };
        m.chipset = self.unprivileged.chipset.clone();
        m.mem.total_bytes = self.unprivileged.mem_total_bytes;
        m.mem.speed_mts = self.privileged.mem_speed_mts;
        m.mem.timings = self.privileged.mem_timings.clone();
        m.mem.parts = self.privileged.mem_parts.clone();
        m.refresh_observed();
        let recomputed = m.hash();
        if !Self::same_hash_shape(&self.machine, &recomputed) {
            // The hash's shape is part of what changed between engine
            // generations; a shape mismatch is not tampering, it is a
            // fingerprint from before the current hash existed.
            return Err(FingerprintError::StaleCapture {
                recorded: self.machine.clone(),
            });
        }
        if recomputed != self.machine {
            return Err(FingerprintError::MachineHashMismatch {
                recorded: self.machine.clone(),
                recomputed,
            });
        }
        Ok(m)
    }

    /// The hash's shape: `<10 chars>-<5 chars or UNKNOWN>`. Any other shape —
    /// including the pre-split single-part hashes — is a previous generation.
    fn same_hash_shape(a: &str, b: &str) -> bool {
        a.len() == b.len() && a.matches('-').count() == b.matches('-').count()
    }

    /// Validate against this host: checksum first, then the unprivileged
    /// block, field by field. Returns the machine the run should record —
    /// the file's, which includes the privileged components an unprivileged
    /// run cannot read for itself.
    pub fn validate(&self) -> Result<Machine, FingerprintError> {
        self.verify_checksum()?;
        let host = Machine::probe();
        self.validate_against(&host)?;
        self.machine()
    }

    /// The comparison half of [`Fingerprint::validate`], split so it can be
    /// tested without this host.
    pub fn validate_against(&self, host: &Machine) -> Result<(), FingerprintError> {
        let differs = |key: &str, want: &str, got: Option<String>| {
            Err(FingerprintError::HardwareChanged {
                field: key.to_owned(),
                expected: want.to_owned(),
                found: got.unwrap_or_else(|| "unreadable".to_owned()),
            })
        };

        if let Some(want) = &self.unprivileged.cpu_model {
            if host.cpu_model.as_deref() != Some(want) {
                return differs("cpu_model", want, host.cpu_model.clone());
            }
        }
        if let Some(want) = self.unprivileged.cpu_base_mhz {
            if host.cpu_base_mhz != Some(want) {
                return differs(
                    "cpu_base_mhz",
                    &want.to_string(),
                    host.cpu_base_mhz.map(|v| v.to_string()),
                );
            }
        }
        if let Some(want) = &self.unprivileged.board_vendor {
            if host.board.vendor.as_deref() != Some(want) {
                return differs("board_vendor", want, host.board.vendor.clone());
            }
        }
        if let Some(want) = &self.unprivileged.board_name {
            if host.board.name.as_deref() != Some(want) {
                return differs("board_name", want, host.board.name.clone());
            }
        }
        if let Some(want) = &self.unprivileged.chipset {
            if host.chipset.as_deref() != Some(want) {
                return differs("chipset", want, host.chipset.clone());
            }
        }
        // A RAM upgrade is a hardware change like any other, so the total is
        // compared too even though it is not a hash component. /proc/meminfo's
        // MemTotal is usable memory and is stable across boots on the same
        // hardware, so exact comparison does not false-positive.
        if let Some(want) = self.unprivileged.mem_total_bytes {
            if host.mem.total_bytes != Some(want) {
                return differs(
                    "mem_total_bytes",
                    &want.to_string(),
                    host.mem.total_bytes.map(|v| v.to_string()),
                );
            }
        }
        Ok(())
    }

    /// Load and checksum-verify the file at `path`.
    pub fn load(path: &Path) -> Result<Self, FingerprintError> {
        let text = std::fs::read_to_string(path).map_err(|e| FingerprintError::Io {
            path: path.to_owned(),
            err: e.to_string(),
        })?;
        let file: Fingerprint = toml::from_str(&text).map_err(|e| FingerprintError::Toml {
            path: path.to_owned(),
            err: e.to_string(),
        })?;
        if file.schema != FINGERPRINT_SCHEMA {
            return Err(FingerprintError::Schema {
                path: path.to_owned(),
                found: file.schema,
            });
        }
        file.verify_checksum().map_err(|e| match e {
            // The checksum error reads better with the path it failed at.
            FingerprintError::Checksum { .. } => FingerprintError::Checksum {
                path: path.to_owned(),
            },
            other => other,
        })?;
        Ok(file)
    }

    /// Write the file, body plus checksum. The checksum covers exactly the
    /// bytes of the body as rendered here, so verification is a re-render.
    /// The checksum line must sit with the other top-level keys, before any
    /// table header — a key after `[table]` belongs to that table.
    pub fn write(&self, path: &Path) -> Result<(), FingerprintError> {
        let checksum = checksum(&self.render_body());
        let mut file = String::new();
        file.push_str(&self.render_head());
        file.push_str(&format!("checksum = {checksum:?}\n"));
        file.push_str(&self.render_tables());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FingerprintError::Io {
                path: path.to_owned(),
                err: e.to_string(),
            })?;
        }
        std::fs::write(path, file).map_err(|e| FingerprintError::Io {
            path: path.to_owned(),
            err: e.to_string(),
        })
    }

    pub fn verify_checksum(&self) -> Result<(), FingerprintError> {
        let want = checksum(&self.render_body());
        if want == self.checksum {
            Ok(())
        } else {
            Err(FingerprintError::Checksum {
                path: path().to_owned(),
            })
        }
    }

    /// The canonical body: every field in a fixed order, `None`s skipped the
    /// same way at write and verify time, strings escaped identically. This
    /// rendering *is* the checksum's input, so it must round-trip: parse a
    /// written file, re-render, and the bytes are the same.
    fn render_body(&self) -> String {
        let mut out = self.render_head();
        out.push_str(&self.render_tables());
        out
    }

    /// Top-level keys, before any table header.
    fn render_head(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("schema = {}\n", self.schema));
        out.push_str(&format!("taken = {}\n", toml_str(&self.taken)));
        out.push_str(&format!("machine = {}\n", toml_str(&self.machine)));
        out
    }

    /// The table sections.
    fn render_tables(&self) -> String {
        let mut out = String::new();
        out.push_str("\n[unprivileged]\n");
        if let Some(v) = &self.unprivileged.cpu_model {
            out.push_str(&format!("cpu_model = {}\n", toml_str(v)));
        }
        if let Some(v) = self.unprivileged.cpu_base_mhz {
            out.push_str(&format!("cpu_base_mhz = {v}\n"));
        }
        if let Some(v) = &self.unprivileged.board_vendor {
            out.push_str(&format!("board_vendor = {}\n", toml_str(v)));
        }
        if let Some(v) = &self.unprivileged.board_name {
            out.push_str(&format!("board_name = {}\n", toml_str(v)));
        }
        if let Some(v) = &self.unprivileged.chipset {
            out.push_str(&format!("chipset = {}\n", toml_str(v)));
        }
        if let Some(v) = self.unprivileged.mem_total_bytes {
            out.push_str(&format!("mem_total_bytes = {v}\n"));
        }
        out.push_str("\n[privileged]\n");
        if let Some(v) = self.privileged.mem_speed_mts {
            out.push_str(&format!("mem_speed_mts = {v}\n"));
        }
        if let Some(v) = &self.privileged.mem_timings {
            out.push_str(&format!("mem_timings = {}\n", toml_str(v)));
        }
        if !self.privileged.mem_parts.is_empty() {
            let parts: Vec<String> = self
                .privileged
                .mem_parts
                .iter()
                .map(|p| toml_str(p))
                .collect();
            out.push_str(&format!("mem_parts = [{}]\n", parts.join(", ")));
        }
        out
    }
}

/// The run-time gate (§9's validation table). Every run calls this before
/// measuring:
///
/// - file present, checksum and hardware agree → the file's machine
/// - file missing → bail with the exact capture command, unless
///   `allow_unfingerprinted`, in which case an unprivileged probe whose hash is
///   degraded by construction (a run without a fingerprint verified less, so it
///   must not share a history with one that did)
///
/// The host a run validated against: the machine to record, and the
/// fingerprint file that vouched for it. `fingerprint` is `None` when the run
/// proceeded unverified — S5 makes the source of trust visible in the run
/// document instead of leaving every run looking equally fingerprinted.
pub struct ValidatedHost {
    pub machine: Machine,
    pub fingerprint: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for ValidatedHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Machine has Debug, but the useful summary here is the hash and the
        // trust source.
        f.debug_struct("ValidatedHost")
            .field("machine_hash", &self.machine.hash())
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

pub fn validate_for_run(allow_unfingerprinted: bool) -> Result<ValidatedHost, String> {
    validate_for_run_at(&path(), allow_unfingerprinted)
}

/// [`validate_for_run`] at an explicit path, split out so the gate's outcomes
/// can be tested without touching this host's /etc.
fn validate_for_run_at(path: &Path, allow_unfingerprinted: bool) -> Result<ValidatedHost, String> {
    if path.exists() {
        let file = Fingerprint::load(path).map_err(|e| e.to_string())?;
        return match file.validate() {
            Ok(machine) => Ok(ValidatedHost {
                machine,
                fingerprint: Some(path.to_owned()),
            }),
            // A capture from an older engine generation is not evidence of
            // tampering — the components still describe some machine, just
            // hashed by a hash this engine no longer computes. An operator who
            // has opted out of fingerprint trust may proceed, degraded.
            // Everything else — checksum, hardware change — is a real
            // mismatch and still bails.
            Err(FingerprintError::StaleCapture { recorded }) if allow_unfingerprinted => {
                eprintln!(
                    "warning: the fingerprint at {} is from an older engine \
                     generation ({recorded}); continuing unverified. This run's \
                     machine hash is degraded and the dataset should reject it.",
                    path.display()
                );
                Ok(ValidatedHost {
                    machine: Machine::probe(),
                    fingerprint: None,
                })
            }
            Err(e) => Err(e.to_string()),
        };
    }
    if allow_unfingerprinted {
        eprintln!(
            "warning: no fingerprint at {}; continuing unverified. This run's \
             machine hash is degraded and the dataset should reject it.",
            path.display()
        );
        return Ok(ValidatedHost {
            machine: Machine::probe(),
            fingerprint: None,
        });
    }
    Err(format!(
        "no machine fingerprint at {}.\nCapture one once per machine:\n    {}\n\
         Or override for this run only: --allow-unfingerprinted",
        path.display(),
        capture_command()
    ))
}

/// A TOML basic string, escaping only what can appear in hardware names.
fn toml_str(raw: &str) -> String {
    format!("\"{}\"", raw.replace('\\', "\\\\").replace('"', "\\\""))
}

/// HMAC-SHA256 over the canonical body, truncated and colon-separated so it
/// reads as bytes rather than one opaque blob.
fn checksum(body: &str) -> String {
    use hmac::Mac;
    use sha2::Sha256;
    let mut mac =
        hmac::Hmac::<Sha256>::new_from_slice(CHECKSUM_KEY).expect("HMAC accepts any key length");
    mac.update(body.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[derive(Debug)]
pub enum FingerprintError {
    Io {
        path: PathBuf,
        err: String,
    },
    Toml {
        path: PathBuf,
        err: String,
    },
    Schema {
        path: PathBuf,
        found: u32,
    },
    /// The body no longer matches its checksum: the file was edited or copied
    /// from another machine and adjusted. Re-capture.
    Checksum {
        path: PathBuf,
    },
    /// The components disagree with the recorded machine hash.
    MachineHashMismatch {
        recorded: String,
        recomputed: String,
    },
    /// The recorded hash has a different width than this engine computes — a
    /// fingerprint captured by an older engine generation, not a forgery.
    StaleCapture {
        recorded: String,
    },
    /// Hardware changed, or the file came from another machine.
    HardwareChanged {
        field: String,
        expected: String,
        found: String,
    },
    NotRoot,
    DmidecodeMissing,
}

impl From<PrivilegedError> for FingerprintError {
    fn from(value: PrivilegedError) -> Self {
        match value {
            PrivilegedError::NotRoot => FingerprintError::NotRoot,
            PrivilegedError::DmidecodeMissing => FingerprintError::DmidecodeMissing,
        }
    }
}

impl std::fmt::Display for FingerprintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FingerprintError::Io { path, err } => write!(f, "{}: {err}", path.display()),
            FingerprintError::Toml { path, err } => {
                write!(f, "{} is not a fingerprint file: {err}", path.display())
            }
            FingerprintError::Schema { path, found } => write!(
                f,
                "{} uses fingerprint schema {found}; this engine knows {}",
                path.display(),
                FINGERPRINT_SCHEMA
            ),
            FingerprintError::Checksum { path } => write!(
                f,
                "checksum mismatch: {} was edited or copied from another machine; \
                 re-capture it with `{}`",
                path.display(),
                capture_command()
            ),
            FingerprintError::MachineHashMismatch {
                recorded,
                recomputed,
            } => write!(
                f,
                "the fingerprint's components hash to {recomputed} but the file \
                 records {recorded}; it was hand-edited"
            ),
            FingerprintError::StaleCapture { recorded } => write!(
                f,
                "the fingerprint records a hash of a previous engine generation \
                 ({recorded}); re-capture it with `{}`",
                capture_command()
            ),
            FingerprintError::HardwareChanged {
                field,
                expected,
                found,
            } => write!(
                f,
                "hardware changed, or this fingerprint came from another machine: \
                 {field} is now `{found}`, the fingerprint says `{expected}`. \
                 Re-capture with `{}`",
                capture_command()
            ),
            FingerprintError::NotRoot => write!(
                f,
                "capturing the privileged block needs root: run `{}`",
                capture_command()
            ),
            FingerprintError::DmidecodeMissing => write!(
                f,
                "dmidecode is not installed; the memory block cannot be captured \
                 without it (apt install dmidecode, or your distro's equivalent)"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Board;

    fn fingerprint() -> Fingerprint {
        let mut machine = Machine::unknown();
        machine.cpu_model = Some("AMD Ryzen 9 9950X 16-Core Processor".into());
        machine.cpu_base_mhz = Some(4300);
        machine.mem.total_bytes = Some(64 * 1024 * 1024 * 1024);
        machine.mem.speed_mts = Some(6000);
        machine.mem.timings = Some("30-36-36-96".into());
        machine.mem.parts = vec!["F5-6000J3036G32GX2-TZ5NR".into()];
        machine.board = Board {
            vendor: Some("ASUSTeK".into()),
            name: Some("ROG STRIX X870E-E".into()),
        };
        machine.chipset = Some("Advanced Micro Devices, Inc. [AMD] Device 1482".into());
        machine.refresh_observed();

        Fingerprint {
            schema: FINGERPRINT_SCHEMA,
            taken: "2026-08-29T11:02:19Z".into(),
            machine: machine.hash(),
            unprivileged: Unprivileged {
                cpu_model: machine.cpu_model.clone(),
                cpu_base_mhz: machine.cpu_base_mhz,
                board_vendor: machine.board.vendor.clone(),
                board_name: machine.board.name.clone(),
                chipset: machine.chipset.clone(),
                mem_total_bytes: machine.mem.total_bytes,
            },
            privileged: Privileged {
                mem_speed_mts: machine.mem.speed_mts,
                mem_timings: machine.mem.timings.clone(),
                mem_parts: machine.mem.parts.clone(),
            },
            checksum: String::new(),
        }
    }

    /// The round-trip the checksum depends on: parse what was written,
    /// re-render, and the body is byte-identical.
    #[test]
    fn written_files_verify() {
        let tmp = std::env::temp_dir().join(format!("sb-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("fingerprint.toml");

        fingerprint().write(&path).unwrap();
        let loaded = Fingerprint::load(&path).unwrap();
        assert_eq!(loaded.machine, fingerprint().machine);
        assert!(loaded.verify_checksum().is_ok());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn the_body_round_trips_through_the_parser() {
        let file = fingerprint();
        let body = file.render_body();
        let parsed: Fingerprint =
            toml::from_str(&format!("{body}checksum = \"{}\"\n", checksum(&body))).unwrap();
        assert_eq!(parsed.render_body(), body);
    }

    /// An edited field invalidates the checksum: that is the whole point of
    /// the field. `machine` is edited here because it is not covered by
    /// validate_against — only the checksum catches it.
    #[test]
    fn an_edit_breaks_the_checksum() {
        let mut file = fingerprint();
        file.checksum = checksum(&file.render_body());
        assert!(file.verify_checksum().is_ok());

        file.machine = "zzzzzz".into();
        assert!(file.verify_checksum().is_err());
    }

    #[test]
    fn hardware_change_is_detected_field_by_field() {
        let file = fingerprint();
        let mut host = file.machine().unwrap();

        assert!(file.validate_against(&host).is_ok());

        host.cpu_model = Some("AMD Ryzen 9 7950X".into());
        match file.validate_against(&host) {
            Err(FingerprintError::HardwareChanged {
                field,
                expected,
                found,
            }) => {
                assert_eq!(field, "cpu_model");
                assert_eq!(expected, "AMD Ryzen 9 9950X 16-Core Processor");
                assert_eq!(found, "AMD Ryzen 9 7950X");
            }
            other => panic!("expected HardwareChanged, got {other:?}"),
        }

        // A component that became unreadable is also a change: hardware did
        // not go away quietly.
        let mut unreadable = file.machine().unwrap();
        unreadable.chipset = None;
        assert!(matches!(
            file.validate_against(&unreadable),
            Err(FingerprintError::HardwareChanged { field, .. }) if field == "chipset"
        ));
    }

    /// A RAM upgrade is a hardware change: the total is compared even though
    /// it is not a hash component.
    #[test]
    fn a_changed_memory_total_is_a_hardware_change() {
        let file = fingerprint();
        let mut host = file.machine().unwrap();
        assert!(file.validate_against(&host).is_ok());

        host.mem.total_bytes = Some(32 * 1024 * 1024 * 1024);
        assert!(matches!(
            file.validate_against(&host),
            Err(FingerprintError::HardwareChanged { field, expected, found })
                if field == "mem_total_bytes"
                    && expected == "68719476736"
                    && found == "34359738368"
        ));

        // Unreadable counts as changed, same as every other field.
        let mut unreadable = file.machine().unwrap();
        unreadable.mem.total_bytes = None;
        assert!(matches!(
            file.validate_against(&unreadable),
            Err(FingerprintError::HardwareChanged { field, .. }) if field == "mem_total_bytes"
        ));
    }

    /// The reconstructed machine carries the memory total, so the run-time
    /// footprint warning works on fingerprinted machines — the check fires
    /// there or nowhere, since the run's machine comes from the file.
    #[test]
    fn machine_reconstruction_carries_memory_total() {
        let machine = fingerprint().machine().unwrap();
        assert_eq!(machine.mem.total_bytes, Some(64 * 1024 * 1024 * 1024));
    }

    /// A degraded fingerprint (timings unreadable at capture) still validates
    /// and still reconstructs a machine whose hash matches.
    #[test]
    fn a_degraded_capture_still_validates() {
        // Captured without decode-dimms: timings absent, so the hash excludes
        // them — and the recorded hash is the degraded one, or the file would
        // disagree with itself.
        let mut machine = fingerprint().machine().unwrap();
        machine.mem.timings = None;
        machine.refresh_observed();
        let mut file = fingerprint();
        file.machine = machine.hash();
        file.privileged.mem_timings = None;
        file.checksum = checksum(&file.render_body());

        let reloaded = file.machine().unwrap();
        assert!(reloaded.is_degraded());
        assert_eq!(reloaded.hash(), file.machine);
        assert!(file.validate_against(&machine).is_ok());
    }

    /// A fingerprint captured by a previous engine generation (hash width
    /// changed under it) is not tampering: checksum intact, host matching.
    /// The gate bails by default — continuity is severed whether we like it or
    /// not — but an operator who has opted out of fingerprint trust may
    /// proceed, degraded. Probed on the real host so the hardware comparison
    /// in validate() passes for the right reason.
    #[test]
    fn a_stale_generation_capture_bails_but_is_overridable() {
        let mut stale = Fingerprint::capture(false).unwrap();
        stale.machine = "abc123".into(); // a previous generation's 6-char width
        stale.checksum = checksum(&stale.render_body());

        let tmp = std::env::temp_dir().join(format!("sb-fp-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("fingerprint.toml");
        stale.write(&path).unwrap();

        let err = validate_for_run_at(&path, false).unwrap_err();
        assert!(err.contains("previous engine generation"), "{err}");

        let host = validate_for_run_at(&path, true).unwrap();
        // S5: an unverified run records no fingerprint source.
        assert!(host.fingerprint.is_none());
        assert!(
            host.machine.is_degraded(),
            "an unverified run hashes what it probed"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
