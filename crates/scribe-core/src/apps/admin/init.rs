//! Digest-owned project scaffolding. Preflight all conflicts before writes,
//! preserve unrelated hook groups and configuration, and reconcile only artifacts
//! whose content still matches the previous installation receipt.

use crate::store::fsio::{self, Owned};
use crate::{Error, Provider, Result, capture::prompt_sha256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

/// One scaffolded artifact and its observed result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InitFile {
    /// Absolute path of the artifact.
    pub path: PathBuf,
    /// `created`, `updated` or `unchanged`.
    pub status: &'static str,
}

/// Concrete scaffolding result; hook trust remains a separate host action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InitReport {
    /// Selected runtime.
    pub provider: Provider,
    /// Existing canonical project directory.
    pub target: PathBuf,
    /// Files handled by this invocation.
    pub files: Vec<InitFile>,
    /// Whether any generated artifact changed.
    pub changed: bool,
    /// Exact remaining runtime steps.
    pub next: Vec<String>,
}

/// Receipt format this binary writes. Format 2 adds `skill_files`.
const RECEIPT_FORMAT: u32 = 2;
/// Receipt formats this binary reads. Format 1 (no `skill_files`) upgrades in place.
const READABLE_RECEIPT_FORMATS: [u32; 2] = [1, 2];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    format: u32,
    skill_sha256: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    skill_files: BTreeMap<String, String>,
    hooks: BTreeMap<String, Value>,
}

/// Only the version of a receipt, read before its shape is checked.
#[derive(Deserialize)]
struct ReceiptFormat {
    format: u32,
}

/// Parse a receipt. The format is checked before the shape, so a receipt from
/// another format is refused as unsupported even when it carries fields this
/// reader does not know.
fn read_receipt(path: &Path, bytes: &[u8]) -> Result<Receipt> {
    let version: ReceiptFormat = serde_json::from_slice(bytes)
        .map_err(|_| conflict(path, "invalid installation receipt"))?;
    if !READABLE_RECEIPT_FORMATS.contains(&version.format) {
        return Err(conflict(path, "unsupported installation receipt format"));
    }
    serde_json::from_slice(bytes).map_err(|_| conflict(path, "invalid installation receipt"))
}

fn conflict(path: &Path, why: &str) -> Error {
    Error::InitConflict {
        path: path.to_path_buf(),
        reason: format!(
            "{why}; preserve your changes and resolve the conflict before rerunning scribe init"
        ),
    }
}

/// Generate the provider's hooks; the binary path is shell quoted, never
/// interpolated unescaped. All events share the same provider-aware CLI.
pub fn integration_hooks(provider: Provider, binary: &Path) -> BTreeMap<String, Value> {
    let quoted = format!("'{}'", binary.to_string_lossy().replace('\'', "'\\''"));
    [("SessionStart", "session-start"), ("UserPromptSubmit", "user-prompt-submit"), ("Stop", "stop"), ("SessionEnd", "session-end")].into_iter().map(|(event, verb)| {
        let timeout = if event == "SessionEnd" { 3 } else { 180 };
        (event.to_owned(), json!({"hooks":[{"type":"command","command":format!("{quoted} --provider {provider} hook {verb}"),"timeout":timeout}]}))
    }).collect()
}

/// Scaffold an explicit existing project. Existing directories may have normal
/// project modes, but every created directory/file is owner-only. Links and
/// writable-by-others paths inside the project are refused.
pub fn init_project(
    target: &Path,
    provider: Provider,
    binary: &Path,
    skill: &str,
) -> Result<InitReport> {
    init_project_with_skill_files(target, provider, binary, skill, &[])
}

/// Scaffold a skill and its bundled files with one ownership receipt.
/// Each additional file has a skill-relative path under a bundle directory.
pub fn init_project_with_skill_files(
    target: &Path,
    provider: Provider,
    binary: &Path,
    skill: &str,
    skill_files: &[(&str, &str)],
) -> Result<InitReport> {
    check_path(target, true)?;
    let target = fsio::canonicalize(target).map_err(|e| Error::io("resolve project", target, e))?;
    let config_dir = target.join(match provider {
        Provider::Codex => ".codex",
        Provider::ClaudeCode => ".claude",
    });
    let skill_dir = target.join(match provider {
        Provider::Codex => ".agents/skills/scribe",
        Provider::ClaudeCode => ".claude/skills/scribe",
    });
    let skill_path = skill_dir.join("SKILL.md");
    let hook_path = config_dir.join(match provider {
        Provider::Codex => "hooks.json",
        Provider::ClaudeCode => "settings.json",
    });
    let receipt_path = config_dir.join("scribe-init.json");
    for path in [&skill_path, &hook_path, &receipt_path] {
        check_descendants(&target, path)?;
    }
    // The project lock is independent of the personal store lock, and held over
    // read/merge/write. The kernel releases it after interruption.
    let _lock = fsio::LockGuard::acquire(
        &target.join(".scribe-init.lock"),
        "scribe init",
        Duration::from_secs(10),
    )?;
    let old = read_optional(&target, &receipt_path)?;
    let receipt: Option<Receipt> = old
        .as_deref()
        .map(|bytes| read_receipt(&receipt_path, bytes))
        .transpose()?;
    let skill_before = read_owned_skill(
        &target,
        &skill_path,
        skill,
        receipt.as_ref().map(|r| r.skill_sha256.as_str()),
    )?;
    let mut digests = receipt
        .as_ref()
        .map_or_else(BTreeMap::new, |r| r.skill_files.clone());
    let mut plans = Vec::new();
    for &(relative, contents) in skill_files {
        let path = skill_dir.join(relative);
        let mut components = Path::new(relative).components();
        let bundle_dir = components.next().and_then(|part| part.as_os_str().to_str());
        if !matches!(
            bundle_dir,
            Some("references" | "scripts" | "assets" | "evals")
        ) || components.next().is_none()
        {
            return Err(conflict(&path, "invalid skill bundle path"));
        }
        check_descendants(&target, &path)?;
        let before = read_owned_skill(
            &target,
            &path,
            contents,
            digests.get(relative).map(String::as_str),
        )?;
        digests.insert(relative.to_owned(), prompt_sha256(contents));
        plans.push((path, contents.as_bytes().to_vec(), before));
    }
    let hook_before = read_optional(&target, &hook_path)?;
    let mut document: Value = match &hook_before {
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|_| conflict(&hook_path, "hook configuration is not JSON"))?,
        None => json!({}),
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| conflict(&hook_path, "hook configuration must be an object"))?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| conflict(&hook_path, "hooks must be an object"))?;
    let expected = integration_hooks(provider, binary);
    for (event, group) in &expected {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| conflict(&hook_path, "hook event must be an array"))?;
        if let Some(previous) = receipt.as_ref().and_then(|r| r.hooks.get(event)) {
            if let Some(n) = groups.iter().position(|g| g == previous) {
                groups.remove(n);
            } else if !groups.contains(group) {
                return Err(conflict(
                    &hook_path,
                    "an owned hook was removed or modified",
                ));
            }
        }
        if !groups.contains(group) {
            groups.push(group.clone());
        }
    }
    let new_receipt = Receipt {
        format: RECEIPT_FORMAT,
        skill_sha256: prompt_sha256(skill),
        skill_files: digests,
        hooks: expected,
    };
    let json_bytes = |v: &Value| -> Result<Vec<u8>> {
        let mut b = serde_json::to_vec_pretty(v).map_err(|e| Error::corrupt(&hook_path, e))?;
        b.push(b'\n');
        Ok(b)
    };
    let receipt_value =
        serde_json::to_value(new_receipt).map_err(|e| Error::corrupt(&receipt_path, e))?;
    // Preserve formatting as well as values when no semantic hook change occurs.
    let hook_bytes = match &hook_before {
        Some(b) if serde_json::from_slice::<Value>(b).ok().as_ref() == Some(&document) => b.clone(),
        _ => json_bytes(&document)?,
    };
    let receipt_bytes = json_bytes(&receipt_value)?;
    // Publish the entry point after every file it links to is in place.
    plans.extend([
        (skill_path, skill.as_bytes().to_vec(), skill_before),
        (hook_path, hook_bytes, hook_before),
        (receipt_path, receipt_bytes, old),
    ]);
    let mut files = Vec::new();
    let mut changed = false;
    for (path, bytes, before) in plans {
        let status = if before.as_ref() == Some(&bytes) {
            "unchanged"
        } else {
            make_parents(&target, &path)?;
            check_descendants(&target, &path)?;
            fsio::write_atomic(&path, &bytes)?;
            changed = true;
            if before.is_some() {
                "updated"
            } else {
                "created"
            }
        };
        files.push(InitFile { path, status });
    }
    let next = match provider {
        Provider::Codex => vec![
            String::from(
                "Restart or resume Codex in this project. Review and trust the current scribe hooks in Settings > Hooks (desktop) or /hooks (CLI).",
            ),
            String::from(
                "Authenticate the Codex CLI and ensure gpt-6-luna is available. Type /scribe on or invoke $scribe on. Skill discovery and scaffolding alone do not enable recording.",
            ),
            String::from(
                "Stop marks catalog passes due; run scribe --provider codex session pass from an authorized scheduler or shell. No scheduler is enabled by init.",
            ),
        ],
        Provider::ClaudeCode => vec![
            String::from(
                "Restart Claude Code in this project and review its hooks. Authenticate the Claude CLI. Type /scribe on to enable cataloging; skill discovery alone does not enable recording.",
            ),
            String::from(
                "Run scribe session pass from an authorized scheduler or shell; init enables no scheduler.",
            ),
        ],
    };
    Ok(InitReport {
        provider,
        target,
        files,
        changed,
        next,
    })
}

fn read_owned_skill(
    root: &Path,
    path: &Path,
    contents: &str,
    digest: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    let before = read_optional(root, path)?;
    if let Some(bytes) = &before
        && bytes != contents.as_bytes()
        && std::str::from_utf8(bytes)
            .ok()
            .is_none_or(|text| Some(prompt_sha256(text).as_str()) != digest)
    {
        return Err(conflict(
            path,
            "skill exists with user-modified or unowned contents",
        ));
    }
    Ok(before)
}

fn read_optional(root: &Path, path: &Path) -> Result<Option<Vec<u8>>> {
    fsio::read_bounded_beneath(root, path, 1024 * 1024)
}

fn check_path(path: &Path, directory: bool) -> Result<()> {
    match fsio::inspect_owned(path, directory)? {
        Owned::Missing | Owned::Fits => Ok(()),
        Owned::WrongType => Err(conflict(path, "path is a link or has the wrong type")),
        Owned::Foreign => Err(conflict(
            path,
            "path is foreign-owned or writable by another user",
        )),
    }
}

fn check_descendants(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| conflict(path, "path escapes project"))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(conflict(path, "path is not normalized"));
        }
        current.push(part);
        check_path(&current, current != path)?;
    }
    Ok(())
}

fn make_parents(root: &Path, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| conflict(path, "missing parent"))?;
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| conflict(path, "path escapes project"))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        if !current.exists() {
            fsio::create_private_dir(&current)?;
        }
        check_path(&current, true)?;
    }
    Ok(())
}
