//! Which build artifacts rtidy knows about, as data rather than as code.
//!
//! rtidy is a Rust tool: Cargo is the only ecosystem that is always on, and the only
//! one with the prevention trick (one shared `target/` dir) behind it. Everything else
//! here is the optional extra — switch "other languages" on in Settings and the same
//! scan, the same preview, the same undo and the same daily task also pick up
//! `node_modules`, `.gradle`, `__pycache__`, `bin`/`obj` and the rest.
//!
//! The reason this is one table instead of a check per module: before it existed,
//! `targets::is_cargo_target`, `discover::is_project` and `config::holds_cargo_project`
//! each spelled out "a `Cargo.toml` next to a `target/`" in their own way, so adding an
//! ecosystem meant editing every one of them and hoping they agreed. They all read this
//! table now, and the walk's skip list is derived from it — a folder is only skipped if
//! no enabled ecosystem wants it.

use std::path::{Path, PathBuf};

/// A re-downloadable or regenerable cache that lives once per user, not per project.
pub struct Store {
    /// Shown in the UI and in `rtidy doctor`.
    pub label: &'static str,
    /// Environment variable that relocates it, when the toolchain has one.
    pub env: Option<&'static str>,
    /// Where it sits when that variable is unset, relative to the home directory.
    pub home_rel: &'static str,
    /// Windows keeps most of these under AppData instead.
    pub win_rel: Option<&'static str>,
}

pub struct Eco {
    /// Stable id, saved in the settings file. Never translate or rename these.
    pub id: &'static str,
    pub label: &'static str,
    /// A directory holding one of these is a project of this kind. `*.ext` matches by
    /// extension; `a/b` is a path below the directory.
    pub manifests: &'static [&'static str],
    /// Directory names, directly inside a project, that hold build output.
    pub artifacts: &'static [&'static str],
    /// Entries that must exist inside an artifact directory for it to count as one.
    /// Empty means the manifest beside it is proof enough.
    pub proof: &'static [&'static str],
    /// Rough rebuild throughput, MB of artifact per second, for the "costs about N s to
    /// rebuild" estimate. Orders of magnitude, not a promise: linking Rust debug output
    /// is slow, unpacking `node_modules` is mostly I/O.
    pub rebuild_mb_s: f64,
    /// Per-user caches this ecosystem fills, which rtidy can measure and trim.
    pub stores: &'static [Store],
}

/// Everything rtidy can recognise, Cargo first.
pub static ALL: [Eco; 17] = [
    Eco {
        id: "cargo",
        label: "Rust (Cargo)",
        manifests: &["Cargo.toml"],
        artifacts: &["target"],
        // A Cargo project can have a source folder called target/; these say the one we
        // found really is build output.
        proof: &["CACHEDIR.TAG", ".rustc_info.json", "debug", "release"],
        // Debug builds are link-bound: a 4 GB target dir is minutes, not seconds.
        rebuild_mb_s: 12.0,
        stores: &[Store { label: "cargo registry", env: Some("CARGO_HOME"), home_rel: ".cargo", win_rel: None }],
    },
    Eco {
        id: "node",
        label: "Node (npm, pnpm, yarn)",
        manifests: &["package.json"],
        artifacts: &["node_modules", ".next", ".nuxt", ".turbo", ".svelte-kit", ".parcel-cache"],
        proof: &[],
        rebuild_mb_s: 40.0,
        stores: &[
            Store { label: "npm cache", env: None, home_rel: ".npm", win_rel: Some("AppData/Local/npm-cache") },
            Store { label: "pnpm store", env: None, home_rel: ".local/share/pnpm/store", win_rel: Some("AppData/Local/pnpm/store") },
            Store { label: "yarn cache", env: None, home_rel: ".cache/yarn", win_rel: Some("AppData/Local/Yarn/Cache") },
        ],
    },
    Eco {
        id: "python",
        label: "Python",
        manifests: &["pyproject.toml", "setup.py", "requirements.txt", "Pipfile"],
        // `.venv` is regenerable but not free; it is listed so it can be *seen*, and the
        // planner holds expensive artifacts back far longer than cheap ones.
        artifacts: &["__pycache__", ".venv", "venv", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".tox", "dist", "build"],
        proof: &[],
        rebuild_mb_s: 25.0,
        stores: &[
            Store { label: "pip cache", env: None, home_rel: ".cache/pip", win_rel: Some("AppData/Local/pip/Cache") },
            Store { label: "uv cache", env: Some("UV_CACHE_DIR"), home_rel: ".cache/uv", win_rel: Some("AppData/Local/uv/cache") },
        ],
    },
    Eco {
        id: "dotnet",
        label: ".NET",
        manifests: &["*.csproj", "*.fsproj", "*.vbproj", "*.sln"],
        artifacts: &["bin", "obj"],
        proof: &[],
        rebuild_mb_s: 30.0,
        stores: &[Store { label: "NuGet packages", env: Some("NUGET_PACKAGES"), home_rel: ".nuget/packages", win_rel: None }],
    },
    Eco {
        id: "gradle",
        label: "Gradle (Java, Kotlin, Android)",
        manifests: &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"],
        artifacts: &["build", ".gradle", ".kotlin"],
        proof: &[],
        rebuild_mb_s: 18.0,
        stores: &[Store { label: "Gradle caches", env: Some("GRADLE_USER_HOME"), home_rel: ".gradle/caches", win_rel: None }],
    },
    Eco {
        id: "maven",
        label: "Maven",
        // Shares the `target` directory name with Cargo; the manifest beside it decides.
        manifests: &["pom.xml"],
        artifacts: &["target"],
        proof: &[],
        rebuild_mb_s: 20.0,
        stores: &[Store { label: "Maven repository", env: None, home_rel: ".m2/repository", win_rel: None }],
    },
    Eco {
        id: "go",
        label: "Go",
        manifests: &["go.mod"],
        // Go keeps nothing per project worth deleting; its caches are all per user.
        artifacts: &[],
        proof: &[],
        rebuild_mb_s: 60.0,
        stores: &[
            Store { label: "Go build cache", env: Some("GOCACHE"), home_rel: ".cache/go-build", win_rel: Some("AppData/Local/go-build") },
            Store { label: "Go module cache", env: Some("GOMODCACHE"), home_rel: "go/pkg/mod", win_rel: None },
        ],
    },
    Eco {
        id: "cmake",
        label: "CMake (C, C++)",
        manifests: &["CMakeLists.txt"],
        artifacts: &["build", "cmake-build-debug", "cmake-build-release", "out"],
        proof: &[],
        rebuild_mb_s: 10.0,
        stores: &[],
    },
    Eco {
        id: "unity",
        label: "Unity",
        manifests: &["ProjectSettings/ProjectVersion.txt", "Assembly-CSharp.csproj"],
        artifacts: &["Library", "Temp", "Obj", "Logs", "Build", "Builds"],
        proof: &[],
        rebuild_mb_s: 15.0,
        stores: &[],
    },
    Eco {
        id: "unreal",
        label: "Unreal Engine",
        manifests: &["*.uproject"],
        artifacts: &["Binaries", "Intermediate", "DerivedDataCache", "Saved"],
        proof: &[],
        rebuild_mb_s: 8.0,
        stores: &[],
    },
    Eco {
        id: "godot",
        label: "Godot",
        manifests: &["project.godot"],
        artifacts: &[".godot", ".import"],
        proof: &[],
        rebuild_mb_s: 35.0,
        stores: &[],
    },
    Eco {
        id: "zig",
        label: "Zig",
        manifests: &["build.zig"],
        artifacts: &["zig-cache", ".zig-cache", "zig-out"],
        proof: &[],
        rebuild_mb_s: 20.0,
        stores: &[Store { label: "Zig cache", env: None, home_rel: ".cache/zig", win_rel: Some("AppData/Local/zig") }],
    },
    Eco {
        id: "elixir",
        label: "Elixir (Mix)",
        manifests: &["mix.exs"],
        artifacts: &["_build", "deps"],
        proof: &[],
        rebuild_mb_s: 25.0,
        stores: &[Store { label: "Hex packages", env: None, home_rel: ".hex/packages", win_rel: Some("AppData/Roaming/hex/packages") }],
    },
    Eco {
        id: "swift",
        label: "Swift (SwiftPM)",
        manifests: &["Package.swift"],
        artifacts: &[".build", ".swiftpm"],
        proof: &[],
        rebuild_mb_s: 14.0,
        stores: &[],
    },
    Eco {
        id: "haskell",
        label: "Haskell (Stack, Cabal)",
        manifests: &["stack.yaml", "*.cabal"],
        artifacts: &[".stack-work", "dist-newstyle"],
        proof: &[],
        rebuild_mb_s: 6.0,
        stores: &[],
    },
    Eco {
        id: "dart",
        label: "Dart, Flutter",
        manifests: &["pubspec.yaml"],
        artifacts: &[".dart_tool", "build"],
        proof: &[],
        rebuild_mb_s: 30.0,
        stores: &[Store { label: "pub cache", env: Some("PUB_CACHE"), home_rel: ".pub-cache", win_rel: Some("AppData/Local/Pub/Cache") }],
    },
    Eco {
        id: "php",
        label: "PHP (Composer)",
        manifests: &["composer.json"],
        artifacts: &["vendor"],
        proof: &[],
        rebuild_mb_s: 40.0,
        stores: &[Store { label: "Composer cache", env: None, home_rel: ".cache/composer", win_rel: Some("AppData/Local/Composer") }],
    },
];

/// Cargo. Always enabled; the rest of rtidy (shared target dir, lean profiles, the
/// cargo download cache) is specific to it.
pub fn cargo() -> &'static Eco {
    &ALL[0]
}

impl Eco {
    /// Does `dir` hold a project of this kind?
    pub fn claims(&self, dir: &Path) -> bool {
        self.manifests.iter().any(|m| match m.strip_prefix("*.") {
            // Extension patterns mean a directory read; only reached for the few
            // ecosystems with no fixed manifest name (.NET, Unreal, Cabal).
            Some(ext) => std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case(ext))),
            None => dir.join(m).exists(),
        })
    }

    /// Seconds to rebuild `bytes` of this ecosystem's output, as an estimate.
    pub fn rebuild_secs(&self, bytes: u64) -> f64 {
        (bytes as f64 / (1 << 20) as f64) / self.rebuild_mb_s
    }
}

/// The ecosystems in play, given the settings. Cargo is always first and always there.
pub fn enabled(all_langs: bool, off: &[String]) -> Vec<&'static Eco> {
    ALL.iter()
        .filter(|e| e.id == "cargo" || (all_langs && !off.iter().any(|o| o == e.id)))
        .collect()
}

/// The ecosystem that owns `dir` as an artifact directory, if an enabled one does.
///
/// The manifest check is what keeps this honest — `build/` is a build directory under a
/// `CMakeLists.txt` and somebody's source folder anywhere else, and `target/` is Cargo
/// next to a `Cargo.toml` and Maven next to a `pom.xml`.
pub fn artifact_of<'a>(dir: &Path, ecos: &[&'a Eco]) -> Option<&'a Eco> {
    let name = dir.file_name()?.to_str()?;
    let parent = dir.parent()?;
    ecos.iter().copied().find(|e| {
        e.artifacts.iter().any(|a| a.eq_ignore_ascii_case(name))
            && (e.proof.is_empty() || e.proof.iter().any(|p| dir.join(p).exists()))
            && e.claims(parent)
    })
}

/// Could any enabled ecosystem have an artifact directory with this name? The walk asks
/// this first, so a manifest check (a `stat`, or a directory read) only happens for the
/// handful of names that could possibly match.
pub fn any_artifact_name(name: &str, ecos: &[&Eco]) -> bool {
    ecos.iter().any(|e| e.artifacts.iter().any(|a| a.eq_ignore_ascii_case(name)))
}

/// Fixed manifest file names of the enabled ecosystems, for noticing projects in passing
/// during a walk. Extension patterns are left out on purpose: they would cost a directory
/// read per folder, and a project with one is still found by its artifacts.
pub fn manifest_names(ecos: &[&Eco]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = ecos
        .iter()
        .flat_map(|e| e.manifests.iter().copied())
        .filter(|m| !m.starts_with("*.") && !m.contains('/'))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The ecosystem with this id, for reading settings and plan files back.
pub fn by_id(id: &str) -> Option<&'static Eco> {
    ALL.iter().find(|e| e.id == id)
}

/// Where a store actually is on this machine, if it is there at all.
pub fn store_path(s: &Store) -> Option<PathBuf> {
    if let Some(var) = s.env
        && let Ok(v) = std::env::var(var)
        && !v.is_empty()
    {
        let p = PathBuf::from(v);
        return p.is_dir().then_some(p);
    }
    let home = dirs::home_dir()?;
    let rel = if cfg!(windows) { s.win_rel.unwrap_or(s.home_rel) } else { s.home_rel };
    let p = rel.split('/').fold(home, |a, part| a.join(part));
    p.is_dir().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_is_always_on_and_others_are_opt_in() {
        let only_rust = enabled(false, &[]);
        assert_eq!(only_rust.len(), 1);
        assert_eq!(only_rust[0].id, "cargo");
        assert_eq!(enabled(true, &[]).len(), ALL.len());
        let without_node = enabled(true, &["node".to_string()]);
        assert!(!without_node.iter().any(|e| e.id == "node"));
        assert!(without_node.iter().any(|e| e.id == "cargo"));
    }

    #[test]
    fn target_is_cargo_or_maven_by_the_manifest_beside_it() {
        let tmp = std::env::temp_dir().join(format!("rtidy-eco-{}", std::process::id()));
        let rust = tmp.join("rust");
        let java = tmp.join("java");
        // A Cargo target dir has to look like one; see `proof`.
        std::fs::create_dir_all(rust.join("target").join("debug")).unwrap();
        std::fs::create_dir_all(java.join("target")).unwrap();
        std::fs::write(rust.join("Cargo.toml"), "").unwrap();
        std::fs::write(java.join("pom.xml"), "").unwrap();
        let ecos = enabled(true, &[]);
        assert_eq!(artifact_of(&rust.join("target"), &ecos).unwrap().id, "cargo");
        assert_eq!(artifact_of(&java.join("target"), &ecos).unwrap().id, "maven");
        // With other languages off, the Maven one is invisible.
        assert!(artifact_of(&java.join("target"), &enabled(false, &[])).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_build_dir_without_a_manifest_is_not_an_artifact() {
        let tmp = std::env::temp_dir().join(format!("rtidy-eco2-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("notes").join("build")).unwrap();
        assert!(artifact_of(&tmp.join("notes").join("build"), &enabled(true, &[])).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
