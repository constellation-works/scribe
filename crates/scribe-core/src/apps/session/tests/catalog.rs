use crate::apps::session::{CatalogBeginInput, Command};
use crate::composition::Context;
use crate::{AuthorName, Provider, SessionId};
use std::path::Path;

const CODEX_ID: &str = "11111111-2222-4333-8444-555555555555";

fn write_private(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn catalog_begin_subject_resolves_codex_session_id_in_codex_only_mode() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let codex_home = dir.path().join("codex");
    let rollout = codex_home
        .join("sessions/2026/11/12")
        .join(format!("rollout-2026-11-12T00-00-00-{CODEX_ID}.jsonl"));
    write_private(
        &rollout,
        &format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{CODEX_ID}\",\"source\":\"vscode\",\"thread_source\":\"user\"}}}}\n"
        ),
    );

    let prefix: SessionId = "11111111".parse().unwrap();
    let author: AuthorName = "opus".parse().unwrap();

    // Codex-only Context (no Claude transcripts directory, provider = Codex).
    let codex_ctx = Context::at(root.clone(), None)
        .with_codex_home(Some(codex_home.clone()))
        .with_provider(Provider::Codex);

    let omitted = Command::CatalogBegin(
        CatalogBeginInput {
            session: prefix.clone(),
            author: author.clone(),
            project: None,
            provider: None,
        },
        codex_ctx.provider(),
    )
    .subject(&codex_ctx);
    assert_eq!(
        omitted.target.as_deref(),
        Some("session:11111111-2222-4333-8444-555555555555")
    );
    assert_eq!(omitted.author.as_deref(), Some("opus"));

    // Even when a Context defaults to ClaudeCode (e.g. dual config), an explicit
    // `provider: Some(Provider::Codex)` still resolves the Codex rollout.
    let default_ctx = Context::at(root, None).with_codex_home(Some(codex_home));
    let explicit = Command::CatalogBegin(
        CatalogBeginInput {
            session: prefix.clone(),
            author: author.clone(),
            project: None,
            provider: Some(Provider::Codex),
        },
        default_ctx.provider(),
    )
    .subject(&default_ctx);
    assert_eq!(
        explicit.target.as_deref(),
        Some("session:11111111-2222-4333-8444-555555555555")
    );

    // Explicit ClaudeCode in Codex-only mode finds no Claude transcripts directory.
    let claude = Command::CatalogBegin(
        CatalogBeginInput {
            session: prefix,
            author,
            project: None,
            provider: Some(Provider::ClaudeCode),
        },
        codex_ctx.provider(),
    )
    .subject(&codex_ctx);
    assert_eq!(claude.target, None);
}
