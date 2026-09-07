//! Resolving the single toolchain a run builds every Rust subject with.
//!
//! Subjects declare a floor rather than a pin, so one run can satisfy a whole
//! selection. Everything inside a run is then comparable by construction, and a
//! rustc upgrade shows up as a difference *between* runs, where it can be
//! faceted, rather than as an uncontrolled confound between two subjects in the
//! same chart.

use crate::manifest::ManifestError;

/// A `major.minor.patch` version, compared numerically.
///
/// String comparison would order 1.100.0 below 1.89.0, which is exactly the kind
/// of quiet wrongness that would pick a toolchain below somebody's floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.trim().split('.').map(|p| p.parse::<u32>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().transpose().ok()?.unwrap_or(0);
        let patch = parts.next().transpose().ok()?.unwrap_or(0);
        if parts.next().is_some() {
            return None;
        }
        Some(Version(major, minor, patch))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The rustc this environment builds with by default, so a resolved toolchain
/// that already matches needs no rustup hop — and a machine without rustup
/// still works at its own version.
pub fn active() -> Option<Version> {
    let out = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()?;
    parse_rustc_version(&String::from_utf8_lossy(&out.stdout))
}

/// `rustc 1.98.0-nightly (0123 … 2026-06-15)` → `1.98.0`. The channel suffix
/// cannot reach [`Version::parse`], and two nightlies of the same number do not
/// agree on enough for a comparison to mean anything anyway.
fn parse_rustc_version(text: &str) -> Option<Version> {
    let raw = text.split_whitespace().nth(1)?;
    Version::parse(raw.split(['-', ' ']).next()?)
}

/// The cargo command that builds with exactly `toolchain` (§4, one toolchain
/// per run, enforced): plain cargo when it is already the active one or
/// unconstrained, else rustup's `cargo +<version>` proxy — the run header
/// records the resolved version, so the build must use exactly it.
pub fn cargo(toolchain: &Version) -> std::process::Command {
    let mut cmd = std::process::Command::new("cargo");
    if *toolchain != Version(0, 0, 0) && Some(*toolchain) != active() {
        cmd.arg(format!("+{toolchain}"));
    }
    cmd
}

/// The toolchain a run should use.
///
/// Without a pin, it is the highest floor among selected subjects — the lowest
/// version that satisfies everyone. With a pin, the pin must clear every floor:
/// a pin below one is refused rather than dropping that subject, because a
/// comparison quietly missing a contender is worse than one that will not start.
pub fn resolve(
    floors: &[(String, Option<String>)],
    pinned: Option<&str>,
) -> Result<Version, ManifestError> {
    let mut highest: Option<(Version, &str)> = None;
    for (subject, raw) in floors {
        let Some(raw) = raw else { continue };
        let Some(version) = Version::parse(raw) else {
            return Err(ManifestError::UnknownKey {
                subject: subject.clone(),
                key: format!("min_rustc: cannot parse `{raw}`"),
            });
        };
        if highest.is_none_or(|(h, _)| version > h) {
            highest = Some((version, subject));
        }
    }

    match pinned {
        None => Ok(highest.map(|(v, _)| v).unwrap_or(Version(0, 0, 0))),
        Some(raw) => {
            let Some(pin) = Version::parse(raw) else {
                return Err(ManifestError::UnknownKey {
                    subject: "<--rustc>".to_owned(),
                    key: format!("cannot parse `{raw}`"),
                });
            };
            if let Some((floor, subject)) = highest {
                if pin < floor {
                    return Err(ManifestError::ToolchainBelowFloor {
                        subject: subject.to_owned(),
                        floor: floor.to_string(),
                        pinned: pin.to_string(),
                    });
                }
            }
            Ok(pin)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_not_lexically() {
        assert!(Version::parse("1.100.0").unwrap() > Version::parse("1.89.0").unwrap());
        assert!(Version::parse("1.89").unwrap() == Version::parse("1.89.0").unwrap());
        assert!(Version::parse("2.0.0").unwrap() > Version::parse("1.99.99").unwrap());
        assert!(Version::parse("1.89.x").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
    }

    fn floors() -> Vec<(String, Option<String>)> {
        vec![
            ("kiddo".into(), Some("1.89.0".into())),
            ("other".into(), Some("1.75.0".into())),
            ("nanoflann".into(), None), // not a Rust subject
        ]
    }

    /// The lowest version that satisfies everyone.
    #[test]
    fn unpinned_resolves_to_the_highest_floor() {
        assert_eq!(resolve(&floors(), None).unwrap(), Version(1, 89, 0));
    }

    #[test]
    fn a_pin_at_or_above_every_floor_is_accepted() {
        assert_eq!(
            resolve(&floors(), Some("1.89.0")).unwrap(),
            Version(1, 89, 0)
        );
        assert_eq!(
            resolve(&floors(), Some("1.92.1")).unwrap(),
            Version(1, 92, 1)
        );
    }

    /// Refused, rather than silently dropping kiddo from the comparison.
    #[test]
    fn a_pin_below_a_floor_is_refused_naming_the_subject() {
        match resolve(&floors(), Some("1.80.0")) {
            Err(ManifestError::ToolchainBelowFloor {
                subject,
                floor,
                pinned,
            }) => {
                assert_eq!(subject, "kiddo");
                assert_eq!(floor, "1.89.0");
                assert_eq!(pinned, "1.80.0");
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn no_rust_subjects_means_no_constraint() {
        let none = vec![("nanoflann".to_string(), None)];
        assert_eq!(resolve(&none, None).unwrap(), Version(0, 0, 0));
        assert_eq!(resolve(&none, Some("1.89.0")).unwrap(), Version(1, 89, 0));
    }

    #[test]
    fn active_rustc_versions_parse_despite_channel_suffixes() {
        assert_eq!(
            parse_rustc_version("rustc 1.98.0-nightly (01dfd7924 2026-06-15)"),
            Some(Version(1, 98, 0))
        );
        assert_eq!(
            parse_rustc_version("rustc 1.89.0 (29tich 2026-06-15)"),
            Some(Version(1, 89, 0))
        );
        assert_eq!(parse_rustc_version("not rustc at all"), None);
    }
}
