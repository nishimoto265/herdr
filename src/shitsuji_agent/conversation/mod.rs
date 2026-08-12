mod claude;
mod codex;
mod readiness;

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) use readiness::{last_completion_checkpoint, CompletionReadiness, FileObservation};

const MAX_TRANSCRIPT_CANDIDATES: usize = 10_000;

fn read_complete_records(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.last() == Some(&b'\n'))
        .map(|line| &line[..line.len() - 1])
}

pub(crate) type ConversationProvider = crate::shitsuji_agent::TranscriptProvider;

impl crate::shitsuji_agent::TranscriptProvider {
    pub(crate) fn from_agent(agent: crate::detect::Agent) -> Option<Self> {
        match agent {
            crate::detect::Agent::Claude => Some(Self::Claude),
            crate::detect::Agent::Codex => Some(Self::Codex),
            _ => None,
        }
    }

    fn transcript_subdirectory(self) -> &'static str {
        match self {
            Self::Claude => "projects",
            Self::Codex => "sessions",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TranscriptCheckpoint {
    pub(crate) byte_offset: u64,
    pub(super) identity: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TranscriptBinding {
    pub(crate) provider: ConversationProvider,
    pub(crate) data_root: PathBuf,
    pub(crate) absolute_path: PathBuf,
    pub(crate) checkpoint: TranscriptCheckpoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptResolution {
    Unique(TranscriptBinding),
    Ambiguous,
    NotFound,
    UnsupportedSchema,
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptQuery<'a> {
    pub(crate) provider: ConversationProvider,
    pub(crate) data_root: &'a Path,
    pub(crate) cwd: &'a Path,
    pub(crate) session_hint: Option<&'a str>,
    /// Files strictly older than this observation are ignored for a manual
    /// session. Managed sessions with an exact hint do not need this filter.
    pub(crate) not_before: Option<SystemTime>,
}

pub(crate) fn default_data_root(provider: ConversationProvider) -> io::Result<PathBuf> {
    match provider {
        ConversationProvider::Claude => crate::integration::claude_dir(),
        ConversationProvider::Codex => crate::integration::codex_dir(),
    }
}

/// Bind one front pane to one provider transcript at the start of a working
/// generation. Multiple plausible same-cwd sessions deliberately fail closed.
pub(crate) fn resolve_transcript(query: &TranscriptQuery<'_>) -> TranscriptResolution {
    let Ok(root) = query.data_root.canonicalize() else {
        return TranscriptResolution::NotFound;
    };
    let search_root = root.join(query.provider.transcript_subdirectory());
    let Ok(search_root) = search_root.canonicalize() else {
        return TranscriptResolution::NotFound;
    };
    if !search_root.starts_with(&root) {
        return TranscriptResolution::NotFound;
    }

    let mut paths = Vec::new();
    if collect_jsonl_files(&search_root, &root, &mut paths).is_err() {
        return TranscriptResolution::NotFound;
    }

    let expected_cwd = query
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| query.cwd.to_path_buf());
    let mut supported = Vec::new();
    let mut saw_unsupported = false;
    for path in paths {
        if query
            .session_hint
            .is_some_and(|hint| !path_contains_session_hint(&path, hint))
        {
            continue;
        }
        if query.session_hint.is_none()
            && query.not_before.is_some_and(|not_before| {
                fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| modified < not_before)
            })
        {
            continue;
        }

        match inspect_candidate(query.provider, &path) {
            Ok(candidate)
                if paths_equal(&candidate.cwd, &expected_cwd)
                    && query
                        .session_hint
                        .is_none_or(|hint| candidate.session_id.as_deref() == Some(hint))
                    && (query.session_hint.is_some()
                        || !is_unhinted_sidechain_candidate(
                            query.provider,
                            &path,
                            candidate.is_sidechain,
                        )) =>
            {
                supported.push((path, candidate));
            }
            Ok(_) => {}
            Err(CandidateError::UnsupportedSchema) => saw_unsupported = true,
            Err(CandidateError::Io) => {}
        }
    }

    match supported.len() {
        0 if saw_unsupported => TranscriptResolution::UnsupportedSchema,
        0 => TranscriptResolution::NotFound,
        1 => {
            let (absolute_path, candidate) = supported.remove(0);
            TranscriptResolution::Unique(TranscriptBinding {
                provider: query.provider,
                data_root: root,
                absolute_path,
                checkpoint: TranscriptCheckpoint {
                    byte_offset: candidate.start_checkpoint_offset,
                    identity: candidate.identity,
                },
            })
        }
        _ => TranscriptResolution::Ambiguous,
    }
}

pub(crate) fn validate_binding(binding: &TranscriptBinding) -> CompletionReadiness {
    let Ok(root) = binding.data_root.canonicalize() else {
        return CompletionReadiness::Unreadable;
    };
    let Ok(metadata) = fs::symlink_metadata(&binding.absolute_path) else {
        return CompletionReadiness::Unreadable;
    };
    let Ok(path) = binding.absolute_path.canonicalize() else {
        return CompletionReadiness::Unreadable;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || !path.starts_with(root) {
        return CompletionReadiness::SessionChanged;
    }
    readiness::read_completion_after(binding)
}

pub(crate) fn file_observation(path: &Path) -> Option<FileObservation> {
    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then_some(FileObservation {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[derive(Debug)]
struct Candidate {
    cwd: PathBuf,
    session_id: Option<String>,
    identity: [u8; 32],
    start_checkpoint_offset: u64,
    is_sidechain: bool,
}

#[derive(Debug)]
enum CandidateError {
    Io,
    UnsupportedSchema,
}

fn inspect_candidate(
    provider: ConversationProvider,
    path: &Path,
) -> Result<Candidate, CandidateError> {
    let bytes = fs::read(path).map_err(|_| CandidateError::Io)?;
    let cwd = match provider {
        ConversationProvider::Claude => claude::transcript_cwd(&bytes),
        ConversationProvider::Codex => codex::transcript_cwd(&bytes),
    }
    .ok_or(CandidateError::UnsupportedSchema)?;
    let session_id = match provider {
        ConversationProvider::Claude => claude::session_id(&bytes),
        ConversationProvider::Codex => codex::session_id(&bytes),
    };
    let start_checkpoint_offset = readiness::checkpoint_before_latest_user(provider, &bytes)
        .unwrap_or_else(|| last_completion_checkpoint(provider, &bytes));
    let identity = readiness::transcript_identity(&bytes, start_checkpoint_offset)
        .ok_or(CandidateError::UnsupportedSchema)?;
    let is_sidechain = match provider {
        ConversationProvider::Claude => claude::is_sidechain_transcript(&bytes),
        ConversationProvider::Codex => false,
    };
    Ok(Candidate {
        cwd,
        session_id,
        identity,
        start_checkpoint_offset,
        is_sidechain,
    })
}

fn is_unhinted_sidechain_candidate(
    provider: ConversationProvider,
    path: &Path,
    record_level_sidechain: bool,
) -> bool {
    provider == ConversationProvider::Claude
        && (record_level_sidechain
            || path
                .components()
                .any(|component| component.as_os_str() == OsStr::new("subagents")))
}

fn collect_jsonl_files(directory: &Path, root: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
    if output.len() >= MAX_TRANSCRIPT_CANDIDATES {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        if output.len() >= MAX_TRANSCRIPT_CANDIDATES {
            break;
        }
        let entry = entry?;
        let file_type = entry.file_type()?;
        // Refusing symlinks prevents a backend assignment from escaping the
        // configured provider root through an attacker-controlled data entry.
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_jsonl_files(&path, root, output)?;
            continue;
        }
        if !file_type.is_file() || path.extension() != Some(OsStr::new("jsonl")) {
            continue;
        }
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if canonical.starts_with(root) {
            output.push(canonical);
        }
    }
    Ok(())
}

fn path_contains_session_hint(path: &Path, hint: &str) -> bool {
    !hint.is_empty()
        && path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.contains(hint))
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
        == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "herdr-shitsuji-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test root");
        path
    }

    fn write_transcript(root: &Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(&path, content).expect("write fixture");
        path
    }

    #[test]
    fn claude_exact_hint_binds_before_current_completed_turn() {
        let root = temp_root("claude-hint");
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_json = serde_json::to_string(&cwd.display().to_string()).expect("json cwd");
        let content = format!(
            "{{\"type\":\"user\",\"cwd\":{cwd_json},\"sessionId\":\"abc\"}}\n{{\"type\":\"assistant\",\"cwd\":{cwd_json},\"message\":{{\"stop_reason\":\"end_turn\"}}}}\n"
        );
        write_transcript(&root, "projects/p/abc.jsonl", &content);

        let result = resolve_transcript(&TranscriptQuery {
            provider: ConversationProvider::Claude,
            data_root: &root,
            cwd: &cwd,
            session_hint: Some("abc"),
            not_before: None,
        });
        let TranscriptResolution::Unique(binding) = result else {
            panic!("expected unique transcript");
        };
        assert_eq!(binding.checkpoint.byte_offset, 0);
        assert_eq!(
            validate_binding(&binding),
            CompletionReadiness::Ready {
                completed: TranscriptCheckpoint {
                    byte_offset: content.len() as u64,
                    identity: binding.checkpoint.identity,
                },
            }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_completed_current_turn_uses_previous_completion_as_checkpoint() {
        let root = temp_root("claude-completed-current");
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_json = serde_json::to_string(&cwd.display().to_string()).expect("json cwd");
        let previous = format!(
            "{{\"type\":\"user\",\"cwd\":{cwd_json},\"sessionId\":\"abc\"}}\n{{\"type\":\"assistant\",\"message\":{{\"stop_reason\":\"end_turn\"}}}}\n"
        );
        let current = format!(
            "{{\"type\":\"user\",\"cwd\":{cwd_json},\"sessionId\":\"abc\"}}\n{{\"type\":\"assistant\",\"message\":{{\"stop_reason\":\"end_turn\"}}}}\n"
        );
        let content = format!("{previous}{current}");
        write_transcript(&root, "projects/p/abc.jsonl", &content);

        let TranscriptResolution::Unique(binding) = resolve_transcript(&TranscriptQuery {
            provider: ConversationProvider::Claude,
            data_root: &root,
            cwd: &cwd,
            session_hint: Some("abc"),
            not_before: None,
        }) else {
            panic!("expected unique transcript");
        };

        assert_eq!(binding.checkpoint.byte_offset, previous.len() as u64);
        assert!(matches!(
            validate_binding(&binding),
            CompletionReadiness::Ready { completed }
                if completed.byte_offset == content.len() as u64
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_completed_current_turn_uses_previous_completion_as_checkpoint() {
        let root = temp_root("codex-completed-current");
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_json = serde_json::to_string(&cwd.display().to_string()).expect("json cwd");
        let metadata = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"abc\",\"cwd\":{cwd_json}}}}}\n"
        );
        let previous = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"previous\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
        );
        let current = concat!(
            "{\"type\":\"response_item\",\"payload\":{\"role\":\"user\",\"content\":[]}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_complete\"}}\n"
        );
        let content = format!("{metadata}{previous}{current}");
        write_transcript(&root, "sessions/2026/01/01/abc.jsonl", &content);

        let TranscriptResolution::Unique(binding) = resolve_transcript(&TranscriptQuery {
            provider: ConversationProvider::Codex,
            data_root: &root,
            cwd: &cwd,
            session_hint: Some("abc"),
            not_before: None,
        }) else {
            panic!("expected unique transcript");
        };

        let expected_checkpoint = metadata.len() + previous.len();
        assert_eq!(binding.checkpoint.byte_offset, expected_checkpoint as u64);
        assert!(matches!(
            validate_binding(&binding),
            CompletionReadiness::Ready { completed }
                if completed.byte_offset == content.len() as u64
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_unhinted_resolution_excludes_subagent_and_sidechain_candidates() {
        let root = temp_root("claude-main-and-subagents");
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_json = serde_json::to_string(&cwd.display().to_string()).expect("json cwd");
        let main = format!("{{\"type\":\"user\",\"cwd\":{cwd_json},\"sessionId\":\"main\"}}\n");
        let sidechain = format!(
            "{{\"type\":\"user\",\"cwd\":{cwd_json},\"sessionId\":\"helper\",\"isSidechain\":true}}\n{{\"type\":\"assistant\",\"cwd\":{cwd_json},\"isSidechain\":true,\"message\":{{\"stop_reason\":\"end_turn\"}}}}\n"
        );
        let main_path = write_transcript(&root, "projects/p/main.jsonl", &main);
        write_transcript(
            &root,
            "projects/p/main/subagents/agent-one.jsonl",
            &sidechain,
        );
        // Record-level sidechain evidence also excludes a copied transcript
        // that is not stored below the conventional subagents directory.
        write_transcript(&root, "projects/p/sidechain-copy.jsonl", &sidechain);

        let TranscriptResolution::Unique(binding) = resolve_transcript(&TranscriptQuery {
            provider: ConversationProvider::Claude,
            data_root: &root,
            cwd: &cwd,
            session_hint: None,
            not_before: None,
        }) else {
            panic!("expected unique main transcript");
        };

        assert_eq!(
            binding.absolute_path,
            main_path.canonicalize().expect("main")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn same_cwd_candidates_fail_closed() {
        let root = temp_root("ambiguous");
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_json = serde_json::to_string(&cwd.display().to_string()).expect("json cwd");
        let record = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"id\",\"cwd\":{cwd_json}}}}}\n"
        );
        write_transcript(&root, "sessions/2026/01/01/a.jsonl", &record);
        write_transcript(&root, "sessions/2026/01/01/b.jsonl", &record);

        assert_eq!(
            resolve_transcript(&TranscriptQuery {
                provider: ConversationProvider::Codex,
                data_root: &root,
                cwd: &cwd,
                session_hint: None,
                not_before: None,
            }),
            TranscriptResolution::Ambiguous
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_transcript_is_not_returned() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let outside = temp_root("outside");
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_json = serde_json::to_string(&cwd.display().to_string()).expect("json cwd");
        let record = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{cwd_json}}}}}\n");
        let outside_file = write_transcript(&outside, "outside.jsonl", &record);
        let link = root.join("sessions/2026/01/01/linked.jsonl");
        fs::create_dir_all(link.parent().expect("link parent")).expect("parent");
        symlink(outside_file, link).expect("symlink");

        assert_eq!(
            resolve_transcript(&TranscriptQuery {
                provider: ConversationProvider::Codex,
                data_root: &root,
                cwd: &cwd,
                session_hint: None,
                not_before: None,
            }),
            TranscriptResolution::NotFound
        );
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn bound_file_replaced_by_symlink_is_rejected_at_readiness() {
        use std::os::unix::fs::symlink;

        let root = temp_root("replace-symlink");
        let outside = temp_root("replace-symlink-outside");
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).expect("cwd");
        let cwd_json = serde_json::to_string(&cwd.display().to_string()).expect("json cwd");
        let record = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{cwd_json}}}}}\n");
        let transcript = write_transcript(&root, "sessions/2026/01/01/session.jsonl", &record);
        let TranscriptResolution::Unique(binding) = resolve_transcript(&TranscriptQuery {
            provider: ConversationProvider::Codex,
            data_root: &root,
            cwd: &cwd,
            session_hint: None,
            not_before: None,
        }) else {
            panic!("binding");
        };
        let outside_file = write_transcript(&outside, "outside.jsonl", &record);
        fs::remove_file(&transcript).expect("remove original");
        symlink(outside_file, transcript).expect("replace with symlink");

        assert_eq!(
            validate_binding(&binding),
            CompletionReadiness::SessionChanged
        );
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn missing_bound_file_is_unreadable() {
        let binding = TranscriptBinding {
            provider: ConversationProvider::Claude,
            data_root: temp_root("missing"),
            absolute_path: temp_root("other-missing").join("gone.jsonl"),
            checkpoint: TranscriptCheckpoint {
                byte_offset: 0,
                identity: [0; 32],
            },
        };
        assert_eq!(validate_binding(&binding), CompletionReadiness::Unreadable);
    }
}
