mod interface;
mod invocation;
mod loading;
mod mentions;
mod model;
mod name_counts;
mod parser;
mod selection;

pub use interface::SkillInterfaceAssetPolicy;
pub use interface::SkillInterfaceFile;
pub use interface::resolve_skill_interface;
pub use invocation::ImplicitSkillAccess;
pub use invocation::ImplicitSkillLookup;
pub use invocation::detect_implicit_skill_invocation_for_command;
pub use invocation::implicit_skill_accesses_for_command;
pub use loading::LoadedSkillRoot;
pub use loading::LoadedSkills;
pub use loading::SkillError;
pub use loading::SkillLoadFuture;
pub use loading::SkillRootLoadRequest;
pub use loading::SkillRootLoader;
pub use loading::SkillRootSnapshotCache;
pub use loading::SkillRootSnapshots;
pub use mentions::ToolMentionKind;
pub use mentions::ToolMentions;
pub use mentions::app_id_from_path;
pub use mentions::extract_tool_mentions;
pub use mentions::extract_tool_mentions_with_sigil;
pub use mentions::normalize_skill_path;
pub use mentions::plugin_config_name_from_path;
pub use mentions::tool_kind_for_path;
pub use model::EnvironmentSkillMetadata;
pub use model::SkillDependencies;
pub use model::SkillInterface;
pub use model::SkillMetadata;
pub use model::SkillPolicy;
pub use model::SkillToolDependency;
pub use name_counts::build_skill_name_counts;
pub use parser::ParsedSkillFrontmatter;
pub use parser::SkillParseError;
pub use parser::parse_skill_frontmatter_metadata;
pub use selection::ExplicitSkillLookup;
pub use selection::collect_explicit_skill_mentions;

use codex_utils_absolute_path::AbsolutePathBuf;
use include_dir::Dir;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::hash::Hash;
use std::hash::Hasher;

use thiserror::Error;

const SYSTEM_SKILLS_DIR: Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/assets/samples");

const SYSTEM_SKILLS_DIR_NAME: &str = ".system";
const SKILLS_DIR_NAME: &str = "skills";
const SYSTEM_SKILLS_MARKER_FILENAME: &str = ".codex-system-skills.marker";
const SYSTEM_SKILLS_MARKER_SALT: &str = "v1";
const SYSTEM_SKILLS_INSTALL_LOCK_FILENAME: &str = ".codex-system-skills.install.lock";
#[cfg(test)]
const SYSTEM_SKILLS_TEST_MODE_ENV: &str = "CODEX_SYSTEM_SKILLS_TEST_MODE";

/// Returns the on-disk cache location for embedded system skills from an absolute CODEX_HOME.
pub fn system_cache_root_dir(codex_home: &AbsolutePathBuf) -> AbsolutePathBuf {
    codex_home
        .join(SKILLS_DIR_NAME)
        .join(SYSTEM_SKILLS_DIR_NAME)
}

/// Installs embedded system skills into `CODEX_HOME/skills/.system`.
///
/// Clears any existing system skills directory first and then writes the embedded
/// skills directory into place.
///
/// To avoid doing unnecessary work on every startup, a marker file is written
/// with a fingerprint of the embedded directory. When the marker matches, the
/// install is skipped.
pub fn install_system_skills(codex_home: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    let skills_root_dir = codex_home.join(SKILLS_DIR_NAME);
    fs::create_dir_all(skills_root_dir.as_path())
        .map_err(|source| SystemSkillsError::io("create skills root dir", source))?;
    #[cfg(test)]
    system_skills_pre_lock_test_hook(codex_home)?;
    let _install_guard = acquire_system_skills_install_lock(codex_home)?;
    #[cfg(test)]
    system_skills_install_test_hook(codex_home)?;

    let dest_system = system_cache_root_dir(codex_home);

    let marker_path = dest_system.join(SYSTEM_SKILLS_MARKER_FILENAME);
    let expected_fingerprint = embedded_system_skills_fingerprint();
    if dest_system.as_path().is_dir()
        && read_marker(&marker_path).is_ok_and(|marker| marker == expected_fingerprint)
    {
        return Ok(());
    }

    if dest_system.as_path().exists() {
        fs::remove_dir_all(dest_system.as_path())
            .map_err(|source| SystemSkillsError::io("remove existing system skills dir", source))?;
    }

    write_embedded_dir(&SYSTEM_SKILLS_DIR, &dest_system)?;
    fs::write(marker_path.as_path(), format!("{expected_fingerprint}\n"))
        .map_err(|source| SystemSkillsError::io("write system skills marker", source))?;
    Ok(())
}

fn acquire_system_skills_install_lock(
    codex_home: &AbsolutePathBuf,
) -> Result<File, SystemSkillsError> {
    let lock_path = codex_home.join(SYSTEM_SKILLS_INSTALL_LOCK_FILENAME);
    if fs::symlink_metadata(lock_path.as_path())
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(SystemSkillsError::io(
            "open system skills install lock",
            std::io::Error::other("refusing to use a symlink system skills install lock"),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path.as_path())
        .map_err(|source| SystemSkillsError::io("open system skills install lock", source))?;
    file.lock()
        .map_err(|source| SystemSkillsError::io("lock system skills install", source))?;
    Ok(file)
}

#[cfg(test)]
fn system_skills_pre_lock_test_hook(codex_home: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    if std::env::var(SYSTEM_SKILLS_TEST_MODE_ENV).ok().as_deref() != Some("probe") {
        return Ok(());
    }
    fs::write(codex_home.join("probe-ready").as_path(), "ready").map_err(|source| {
        SystemSkillsError::io("write system skills test probe-ready marker", source)
    })?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !codex_home.join("allow-probe").as_path().is_file() {
        if std::time::Instant::now() >= deadline {
            return Err(SystemSkillsError::io(
                "wait for system skills test probe release",
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "parent did not release system skills test probe",
                ),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(test)]
fn system_skills_install_test_hook(codex_home: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    match std::env::var(SYSTEM_SKILLS_TEST_MODE_ENV).ok().as_deref() {
        Some("hold") => {
            fs::write(codex_home.join("lock-held").as_path(), "held").map_err(|source| {
                SystemSkillsError::io("write system skills test lock marker", source)
            })?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !codex_home.join("release-lock").as_path().is_file() {
                if std::time::Instant::now() >= deadline {
                    return Err(SystemSkillsError::io(
                        "wait for system skills test lock release",
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "parent did not release system skills test lock",
                        ),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        Some("probe") => {
            fs::write(codex_home.join("probe-entered").as_path(), "entered").map_err(|source| {
                SystemSkillsError::io("write system skills test probe marker", source)
            })?;
        }
        _ => {}
    }
    Ok(())
}

fn read_marker(path: &AbsolutePathBuf) -> Result<String, SystemSkillsError> {
    Ok(fs::read_to_string(path.as_path())
        .map_err(|source| SystemSkillsError::io("read system skills marker", source))?
        .trim()
        .to_string())
}

fn embedded_system_skills_fingerprint() -> String {
    let mut items = Vec::new();
    collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
    items.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

    let mut hasher = DefaultHasher::new();
    SYSTEM_SKILLS_MARKER_SALT.hash(&mut hasher);
    for (path, contents_hash) in items {
        path.hash(&mut hasher);
        contents_hash.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

fn collect_fingerprint_items(dir: &Dir<'_>, items: &mut Vec<(String, Option<u64>)>) {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                items.push((subdir.path().to_string_lossy().to_string(), None));
                collect_fingerprint_items(subdir, items);
            }
            include_dir::DirEntry::File(file) => {
                let mut file_hasher = DefaultHasher::new();
                file.contents().hash(&mut file_hasher);
                items.push((
                    file.path().to_string_lossy().to_string(),
                    Some(file_hasher.finish()),
                ));
            }
        }
    }
}

/// Writes the embedded `include_dir::Dir` to disk under `dest`.
///
/// Preserves the embedded directory structure.
fn write_embedded_dir(dir: &Dir<'_>, dest: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    fs::create_dir_all(dest.as_path())
        .map_err(|source| SystemSkillsError::io("create system skills dir", source))?;

    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                let subdir_dest = dest.join(subdir.path());
                fs::create_dir_all(subdir_dest.as_path()).map_err(|source| {
                    SystemSkillsError::io("create system skills subdir", source)
                })?;
                write_embedded_dir(subdir, dest)?;
            }
            include_dir::DirEntry::File(file) => {
                let path = dest.join(file.path());
                if let Some(parent) = path.as_path().parent() {
                    fs::create_dir_all(parent).map_err(|source| {
                        SystemSkillsError::io("create system skills file parent", source)
                    })?;
                }
                fs::write(path.as_path(), file.contents())
                    .map_err(|source| SystemSkillsError::io("write system skill file", source))?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum SystemSkillsError {
    #[error("io error while {action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl SystemSkillsError {
    fn io(action: &'static str, source: std::io::Error) -> Self {
        Self::Io { action, source }
    }
}

#[cfg(test)]
mod tests {
    use super::SYSTEM_SKILLS_DIR;
    use super::SYSTEM_SKILLS_TEST_MODE_ENV;
    use super::collect_fingerprint_items;
    use super::install_system_skills;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use std::env;
    use std::fs;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    const SYSTEM_SKILLS_CHILD_HOME_ENV: &str = "CODEX_SYSTEM_SKILLS_CHILD_HOME";
    const SYSTEM_SKILLS_CHILD_MODE_ENV: &str = "CODEX_SYSTEM_SKILLS_CHILD_MODE";

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after Unix epoch")
                .as_nanos();
            let root = env::temp_dir().join(format!("{prefix}-{nonce}"));
            fs::create_dir_all(&root).expect("temporary system skills root is created");
            Self(root)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct ChildGuard(Vec<std::process::Child>);

    impl ChildGuard {
        fn spawn(&mut self, command: &mut Command) {
            self.0
                .push(command.spawn().expect("system skills child process starts"));
        }

        fn wait_successes_before(&mut self, deadline: std::time::Instant) {
            for child in &mut self.0 {
                loop {
                    if let Some(status) = child
                        .try_wait()
                        .expect("system skills child process remains waitable")
                    {
                        assert!(
                            status.success(),
                            "concurrent system skills child install succeeds"
                        );
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "system skills child install completes before deadline"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            for child in &mut self.0 {
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }

    fn wait_for_file(path: &std::path::Path, deadline: std::time::Instant, message: &str) {
        while !path.is_file() {
            assert!(std::time::Instant::now() < deadline, "{message}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn fingerprint_traverses_nested_entries() {
        let mut items = Vec::new();
        collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
        let mut paths: Vec<String> = items.into_iter().map(|(path, _)| path).collect();
        paths.sort_unstable();

        assert!(
            paths
                .binary_search_by(|probe| probe.as_str().cmp("skill-creator/SKILL.md"))
                .is_ok()
        );
        assert!(
            paths
                .binary_search_by(|probe| probe.as_str().cmp("skill-creator/scripts/init_skill.py"))
                .is_ok()
        );
    }

    #[test]
    fn concurrent_system_skill_installs_succeed() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("codex-system-skills-{nonce}"));
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let codex_home =
                        AbsolutePathBuf::try_from(root).expect("temporary path is absolute");
                    barrier.wait();
                    install_system_skills(&codex_home)
                })
            })
            .collect();

        for handle in handles {
            handle
                .join()
                .expect("system skill install thread completes")
                .expect("concurrent embedded system skills install succeeds");
        }

        assert!(
            root.join("skills")
                .join(".system")
                .join("skill-creator")
                .join("scripts")
                .join("init_skill.py")
                .is_file()
        );

        fs::remove_dir_all(root).expect("temporary system skills root is removed");
    }

    #[test]
    fn concurrent_process_system_skill_installs_succeed() {
        let root = TestDir::new("codex-system-skills-process");
        let current_exe = env::current_exe().expect("current test executable is available");
        let mut children = ChildGuard::default();
        for _ in 0..8 {
            let mut command = Command::new(&current_exe);
            command
                .args([
                    "--exact",
                    "tests::system_skill_install_child",
                    "--nocapture",
                ])
                .env(SYSTEM_SKILLS_CHILD_HOME_ENV, root.path())
                .env(SYSTEM_SKILLS_CHILD_MODE_ENV, "barrier");
            children.spawn(&mut command);
        }

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while fs::read_dir(root.path())
            .expect("temporary system skills root remains readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("ready-"))
            .count()
            < children.0.len()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "all system skills child processes should reach the start barrier"
            );
            thread::sleep(Duration::from_millis(10));
        }
        fs::write(root.path().join("start"), "go").expect("child start barrier is released");
        children.wait_successes_before(deadline);
        assert!(
            root.path()
                .join("skills")
                .join(".system")
                .join("skill-creator")
                .join("scripts")
                .join("init_skill.py")
                .is_file()
        );
    }

    #[test]
    fn system_skill_install_lock_blocks_a_second_process() {
        let root = TestDir::new("codex-system-skills-lock");
        let current_exe = env::current_exe().expect("current test executable is available");
        let mut children = ChildGuard::default();
        let mut holder = Command::new(&current_exe);
        holder
            .args([
                "--exact",
                "tests::system_skill_install_child",
                "--nocapture",
            ])
            .env(SYSTEM_SKILLS_CHILD_HOME_ENV, root.path())
            .env(SYSTEM_SKILLS_CHILD_MODE_ENV, "hold")
            .env(SYSTEM_SKILLS_TEST_MODE_ENV, "hold");
        children.spawn(&mut holder);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        wait_for_file(
            &root.path().join("lock-held"),
            deadline,
            "first process should acquire the system skills install lock",
        );

        let mut probe = Command::new(&current_exe);
        probe
            .args([
                "--exact",
                "tests::system_skill_install_child",
                "--nocapture",
            ])
            .env(SYSTEM_SKILLS_CHILD_HOME_ENV, root.path())
            .env(SYSTEM_SKILLS_CHILD_MODE_ENV, "probe")
            .env(SYSTEM_SKILLS_TEST_MODE_ENV, "probe");
        children.spawn(&mut probe);
        wait_for_file(
            &root.path().join("probe-ready"),
            deadline,
            "second process should reach the pre-lock probe barrier",
        );
        fs::write(root.path().join("allow-probe"), "go")
            .expect("second process is released to acquire the system skills install lock");
        let probe_deadline = std::time::Instant::now() + Duration::from_millis(250);
        while std::time::Instant::now() < probe_deadline {
            assert!(
                !root.path().join("probe-entered").is_file(),
                "second process entered the installer while the first process held its lock"
            );
            thread::sleep(Duration::from_millis(10));
        }
        fs::write(root.path().join("release-lock"), "release")
            .expect("system skills lock holder is released");
        children.wait_successes_before(deadline);
        assert!(
            root.path().join("probe-entered").is_file(),
            "second process enters after the first releases the system skills install lock"
        );
    }

    #[test]
    fn system_skill_install_child() {
        let Some(root) = env::var_os(SYSTEM_SKILLS_CHILD_HOME_ENV) else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        if env::var(SYSTEM_SKILLS_CHILD_MODE_ENV).ok().as_deref() == Some("barrier") {
            fs::write(root.join(format!("ready-{}", std::process::id())), "ready")
                .expect("child reaches system skills start barrier");
            wait_for_file(
                &root.join("start"),
                std::time::Instant::now() + Duration::from_secs(10),
                "parent should release the system skills child start barrier",
            );
        }
        let codex_home = AbsolutePathBuf::try_from(root).expect("temporary path is absolute");
        install_system_skills(&codex_home).expect("embedded system skills install succeeds");
    }
}
