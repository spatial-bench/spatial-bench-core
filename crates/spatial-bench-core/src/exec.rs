//! The exec adapter's builders: how a non-rust driver gets built
//! and invoked, per language.
//!
//! The harness contract is identical for every language — `--list`, a RunSpec
//! as JSON on stdin, JSONL points on stdout — so the only per-language thing
//! here is the build, cached per content hash. A run that never selects a
//! subject never builds it; selecting one builds exactly its environment.

use crate::manifest::{Build, Source};
use std::path::{Path, PathBuf};

/// What an exec subject's manifest declares, resolved for one build.
pub struct ExecInputs {
    /// The subject's manifest directory — the anchor for the entry file.
    pub manifest_dir: PathBuf,
    /// `cxx` or `python` (validated at manifest load).
    pub lang: String,
    /// The driver's entry file, resolved against `manifest_dir`.
    pub entry: PathBuf,
    /// The manifest's build recipe.
    pub build: Build,
    /// The manifest's source pin.
    pub source: Source,
    /// The expected library sha , when the manifest declares one. The cxx
    /// build verifies the fetched sources against it.
    pub expected_sha: Option<String>,
}

/// What a successful exec build produced.
pub struct Built {
    /// The argv to run the driver: `[binary]` for cxx,
    /// `[venv python, driver.py]` for python. The harness contract's transport
    /// is the same either way.
    pub program: Vec<String>,
    /// The library revision actually built against — the resolved git commit
    /// sha for cxx. `None` when the pin is a package version (python), whose
    /// immutability is the provenance .
    pub resolved_sha: Option<String>,
    /// How many compile-time specialisations the binary carries (the dims
    /// list for cxx; one for python, which dispatches at run time).
    pub combinations: usize,
    pub cache_key: String,
}

/// Build (or reuse) the driver environment for an exec subject.
///
/// Everything lands under `<build_root>/<subject>/exec/<cache_key>/` — engine
/// output, never sources — and the library sources under
/// `<build_root>/<subject>/sources/<resolved sha>/`, immutable per sha.
pub fn prepare(
    subject: &str,
    inputs: &ExecInputs,
    build_root: &Path,
) -> Result<(Built, PathBuf), String> {
    match inputs.lang.as_str() {
        "cxx" => prepare_cxx(subject, inputs, build_root),
        "python" => prepare_python(subject, inputs, build_root),
        other => Err(format!(
            "exec language `{other}` is not one of cxx, python — the manifest \
             loader should have caught this"
        )),
    }
}

fn prepare_cxx(
    subject: &str,
    inputs: &ExecInputs,
    build_root: &Path,
) -> Result<(Built, PathBuf), String> {
    // The library sources, fetched at the pin. Keyed by the resolved sha, so
    // a ref that moved cannot silently reuse (or poison) an old checkout.
    let (lib_dir, resolved_sha) = fetch_lib_source(
        subject,
        &inputs.source,
        inputs.expected_sha.as_deref(),
        &build_root.join(subject).join("sources"),
    )?;
    let cmake_dir = inputs
        .build
        .cmake
        .as_ref()
        .map(|recipe| build_cmake_subject(subject, &lib_dir, recipe, build_root))
        .transpose()?;

    // The compiler is part of what is built, so part of the cache key.
    let compiler = std::process::Command::new("g++")
        .arg("--version")
        .output()
        .map_err(|e| format!("could not run g++: {e}"))?;
    if !compiler.status.success() {
        return Err("g++ --version failed; a C++ toolchain is required for \
                    cxx exec subjects"
            .to_owned());
    }
    let compiler_line = String::from_utf8_lossy(&compiler.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();

    let shim = std::fs::read_to_string(&inputs.entry).map_err(|e| {
        format!(
            "{}: cannot read the exec entry: {e}",
            inputs.entry.display()
        )
    })?;

    let mut key_material = String::new();
    key_material.push_str("cxx-shim\u{1}");
    key_material.push_str(&shim);
    key_material.push('\u{1}');
    key_material.push_str(inputs.build.std.as_deref().unwrap_or(""));
    key_material.push('\u{1}');
    key_material.push_str(&inputs.build.flags.join("\u{1}"));
    key_material.push('\u{1}');
    key_material.push_str(&inputs.build.include.join("\u{1}"));
    if let Some(recipe) = &inputs.build.cmake {
        key_material.push('\u{1}');
        key_material.push_str(&recipe.configure.join("\u{1}"));
        key_material.push('\u{1}');
        key_material.push_str(recipe.target.as_deref().unwrap_or(""));
        key_material.push('\u{1}');
        key_material.push_str(&recipe.include.join("\u{1}"));
        key_material.push('\u{1}');
        key_material.push_str(&recipe.libraries.join("\u{1}"));
        for dependency in &recipe.dependency {
            key_material.push('\u{1}');
            key_material.push_str(&dependency.name);
            key_material.push('\u{1}');
            key_material.push_str(&dependency.repo);
            key_material.push('\u{1}');
            key_material.push_str(&dependency.pinned_ref);
            key_material.push('\u{1}');
            key_material.push_str(&dependency.sha);
            key_material.push('\u{1}');
            key_material.push_str(&dependency.cmake_var);
        }
    }
    key_material.push('\u{1}');
    key_material.push_str(
        &inputs
            .build
            .compile_time_dims
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
    key_material.push('\u{1}');
    key_material.push_str(&resolved_sha);
    key_material.push('\u{1}');
    key_material.push_str(&compiler_line);
    let cache_key = blake3::hash(key_material.as_bytes()).to_hex()[..16].to_owned();

    let dir = build_root.join(subject).join("exec").join(&cache_key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    // The dims dispatch header: the shim includes it and dispatches on the
    // dimensionality of each case, so one binary carries every
    // specialisation the manifest's compile_time_dims declares.
    let dims = if inputs.build.compile_time_dims.is_empty() {
        vec![3]
    } else {
        inputs.build.compile_time_dims.clone()
    };
    write_if_changed(&dir.join("dims.hpp"), &dims_hpp(&dims))?;

    let binary = dir.join("driver");
    if !binary.exists() {
        let mut cmd = std::process::Command::new("g++");
        cmd.arg(format!(
            "-std={}",
            inputs.build.std.as_deref().unwrap_or("c++17")
        ));
        for flag in &inputs.build.flags {
            cmd.arg(flag);
        }
        for inc in &inputs.build.include {
            cmd.arg(format!("-I{}", lib_dir.join(inc).display()));
        }
        if let (Some(recipe), Some(cmake_dir)) = (&inputs.build.cmake, &cmake_dir) {
            for inc in &recipe.include {
                cmd.arg(format!("-I{}", cmake_dir.join(inc).display()));
            }
        }
        cmd.arg("-I").arg(&dir);
        cmd.arg(&inputs.entry).arg("-o").arg(&binary);
        if let (Some(recipe), Some(cmake_dir)) = (&inputs.build.cmake, &cmake_dir) {
            for library in &recipe.libraries {
                cmd.arg(cmake_dir.join(library));
            }
        }
        let out = cmd
            .output()
            .map_err(|e| format!("could not run g++: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "compiling the {subject} shim failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }

    Ok((
        Built {
            program: vec![binary.display().to_string()],
            resolved_sha: Some(resolved_sha),
            combinations: dims.len(),
            cache_key,
        },
        dir,
    ))
}

fn build_cmake_subject(
    subject: &str,
    lib_dir: &Path,
    recipe: &crate::manifest::CmakeBuild,
    build_root: &Path,
) -> Result<PathBuf, String> {
    let source = lib_dir.join(
        recipe
            .source_dir
            .as_deref()
            .unwrap_or_else(|| Path::new("")),
    );
    let build = build_root.join(subject).join("cmake");
    let dependency_root = build_root.join(subject).join("dependencies");
    let mut configure = std::process::Command::new("cmake");
    configure.args([
        "-S",
        &source.display().to_string(),
        "-B",
        &build.display().to_string(),
        "-G",
        "Ninja",
    ]);
    for dependency in &recipe.dependency {
        let source = Source {
            kind: "git".to_owned(),
            repo: Some(dependency.repo.clone()),
            package: None,
            pinned_ref: dependency.pinned_ref.clone(),
            sha: Some(dependency.sha.clone()),
        };
        let (path, _) = fetch_lib_source(
            &format!("{subject}-{}", dependency.name),
            &source,
            Some(&dependency.sha),
            &dependency_root.join(&dependency.name),
        )?;
        configure.arg(format!("-D{}={}", dependency.cmake_var, path.display()));
    }
    let configured = configure
        .args(&recipe.configure)
        .output()
        .map_err(|e| format!("could not configure {subject} with cmake: {e}"))?;
    if !configured.status.success() {
        return Err(format!(
            "configuring {subject} from pinned source failed:\n{}",
            String::from_utf8_lossy(&configured.stderr)
        ));
    }
    let mut command = std::process::Command::new("cmake");
    command.args(["--build", &build.display().to_string()]);
    if let Some(target) = &recipe.target {
        command.args(["--target", target]);
    }
    let built = command
        .output()
        .map_err(|e| format!("could not build {subject} with cmake: {e}"))?;
    if !built.status.success() {
        return Err(format!(
            "building {subject} from pinned source failed:\n{}",
            String::from_utf8_lossy(&built.stderr)
        ));
    }
    Ok(build)
}

fn prepare_python(
    subject: &str,
    inputs: &ExecInputs,
    build_root: &Path,
) -> Result<(Built, PathBuf), String> {
    let driver = std::fs::read_to_string(&inputs.entry).map_err(|e| {
        format!(
            "{}: cannot read the exec entry: {e}",
            inputs.entry.display()
        )
    })?;
    let python_version = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map_err(|e| format!("could not run python3: {e}"))?;
    let python_line = String::from_utf8_lossy(&python_version.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();

    let pin = &inputs.source.pinned_ref;
    let mut key_material = String::new();
    key_material.push_str("python-driver\u{1}");
    key_material.push_str(&driver);
    key_material.push('\u{1}');
    key_material.push_str(&inputs.source.kind);
    key_material.push('\u{1}');
    key_material.push_str(inputs.source.package.as_deref().unwrap_or(""));
    key_material.push('\u{1}');
    key_material.push_str(pin);
    key_material.push('\u{1}');
    key_material.push_str(&python_line);
    let cache_key = blake3::hash(key_material.as_bytes()).to_hex()[..16].to_owned();

    let dir = build_root.join(subject).join("exec").join(&cache_key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let venv_python = dir.join("venv/bin/python");

    if !venv_python.exists() {
        let venv = std::process::Command::new("python3")
            .args(["-m", "venv"])
            .arg(dir.join("venv"))
            .output()
            .map_err(|e| format!("could not run python3 -m venv: {e}"))?;
        if !venv.status.success() {
            return Err(format!(
                "creating the {subject} venv failed:\n{}",
                String::from_utf8_lossy(&venv.stderr)
            ));
        }
    }

    // Install the pinned library. A PyPI version is immutable once published,
    // which is the python side of the S1 pin story; a git pin installs the
    // exact ref. A subject with no library (pure-stdlib driver) installs
    // nothing.
    let pip = dir.join("venv/bin/pip");
    let installed = match inputs.source.kind.as_str() {
        "pypi" => {
            let package =
                inputs.source.package.as_deref().ok_or_else(|| {
                    format!("{subject}: source kind pypi declares no package name")
                })?;
            format!("{package}=={pin}")
        }
        "git" => {
            let repo = inputs
                .source
                .repo
                .as_deref()
                .ok_or_else(|| format!("{subject}: source kind git declares no repo"))?;
            format!("git+{repo}@{pin}")
        }
        _ => String::new(),
    };
    if !installed.is_empty() {
        let marker = dir.join(".installed");
        let want = format!("{installed}\n");
        if std::fs::read_to_string(&marker).unwrap_or_default() != want {
            let out = std::process::Command::new(&pip)
                .args(["install", "--quiet", &installed])
                .output()
                .map_err(|e| format!("could not run pip: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "installing {installed} for {subject} failed:\n{}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            std::fs::write(&marker, want).map_err(|e| e.to_string())?;
        }
    }

    Ok((
        Built {
            program: vec![
                venv_python.display().to_string(),
                inputs.entry.display().to_string(),
            ],
            resolved_sha: None,
            combinations: 1,
            cache_key,
        },
        dir,
    ))
}

/// Fetch the library sources at the manifest's pin, returning the checkout
/// directory and the resolved commit sha. Keyed by the resolved sha: a moved
/// ref lands in a different directory and is caught by the sha check .
fn fetch_lib_source(
    subject: &str,
    source: &Source,
    expected_sha: Option<&str>,
    sources_root: &Path,
) -> Result<(PathBuf, String), String> {
    if source.kind != "git" {
        return Err(format!(
            "{subject}: cxx exec subjects fetch their library from a git pin; \
             source kind `{}` is not supported yet",
            source.kind
        ));
    }
    let repo = source
        .repo
        .as_deref()
        .ok_or_else(|| format!("{subject}: source kind git declares no repo"))?;

    let tmp = sources_root.join(format!("tmp-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let ref_arg = source.pinned_ref.clone();
    let is_sha = crate::build::is_commit_id(&ref_arg);

    let run = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("git")
            .args(args)
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };

    if is_sha {
        run(&["init", "-q", tmp.to_str().ok_or("non-utf8 build path")?])?;
        run(&[
            "-C",
            tmp.to_str().ok_or("non-utf8 build path")?,
            "remote",
            "add",
            "origin",
            repo,
        ])?;
        run(&[
            "-C",
            tmp.to_str().ok_or("non-utf8 build path")?,
            "fetch",
            "-q",
            "--depth",
            "1",
            "origin",
            &ref_arg,
        ])
        .map_err(|e| format!("{subject}: fetching the pinned sha failed: {e}"))?;
        run(&[
            "-C",
            tmp.to_str().ok_or("non-utf8 build path")?,
            "checkout",
            "-q",
            "FETCH_HEAD",
        ])?;
    } else {
        run(&[
            "clone",
            "-q",
            "--depth",
            "1",
            "--branch",
            &ref_arg,
            repo,
            tmp.to_str().ok_or("non-utf8 build path")?,
        ])
        .map_err(|e| format!("{subject}: cloning the pinned ref failed: {e}"))?;
    }
    let resolved = run(&[
        "-C",
        tmp.to_str().ok_or("non-utf8 build path")?,
        "rev-parse",
        "HEAD",
    ])?;

    if let Some(expected) = expected_sha {
        if resolved != expected {
            return Err(format!(
                "{subject}: the pin moved — the manifest expects {expected} but \
                 the library's ref resolves to {resolved}. If the new revision is \
                 intended, update the manifest's pinned_ref and sha together."
            ));
        }
    }

    let dir = sources_root.join(&resolved);
    if !dir.exists() {
        std::fs::rename(&tmp, &dir)
            .map_err(|e| format!("{} → {}: {e}", tmp.display(), dir.display()))?;
    } else {
        std::fs::remove_dir_all(&tmp).ok();
    }
    Ok((dir, resolved))
}

/// The dims dispatch header the shim includes: one specialisation per value
/// in the manifest's compile_time_dims, selected at run time by the case's
/// `dims` tag.
fn dims_hpp(dims: &[u32]) -> String {
    let mut out = String::from(
        "// Generated by spatial-bench exec build — do not edit.\n\
         // The dimensionality specialisations this binary carries.\n\
         #pragma once\n\
         #include <cstdint>\n\
         #include <type_traits>\n\
         template <typename F>\n\
         inline bool dispatch_dims(unsigned dims, F&& f) {\n\
         \x20   switch (dims) {\n",
    );
    for d in dims {
        out.push_str(&format!(
            "        case {d}: {{ f(std::integral_constant<int, {d}>{{}}); return true; }}\n"
        ));
    }
    out.push_str("    }\n    return false;\n}\n");
    out
}

/// Only touch a file whose content differs, mirroring build.rs's rule: an
/// unchanged file keeps its mtime, so a cached build stays cached.
fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).unwrap_or_default() == content {
        return Ok(());
    }
    std::fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
}
