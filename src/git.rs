//! Git por argv (nunca shell): stdin nulo, tempo e saída limitados, sem `--force`.
//! Worktrees por tarefa e captura das mudanças de cada turno.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::persistence::{Database, DbError};

const TIMEOUT: Duration = Duration::from_secs(30);
/// Hooks de pre-commit (testes, lint) podem demorar bem mais que um comando normal.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(300);
/// Arquivo não rastreado maior que isso fica fora do snapshot (não é lido nem gravado no repositório).
const SNAPSHOT_FILE_CAP: u64 = 1024 * 1024;
const OUT_CAP: usize = 2 * 1024 * 1024;
/// Teto de um patch por arquivo e de arquivos listados por turno.
const FILE_PATCH_CAP: usize = 256 * 1024;
const MAX_FILES: usize = 200;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("Falha ao executar git: {0}")]
    Io(#[from] std::io::Error),

    #[error("git {args} falhou: {stderr}")]
    Failed { args: String, stderr: String },

    #[error("git excedeu {0}s")]
    Timeout(u64),

    #[error("Nome de branch inválido: {0}")]
    InvalidBranch(String),

    #[error("{0}")]
    Other(String),

    #[error(transparent)]
    Db(#[from] DbError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatus {
    /// Coluna X do `status --porcelain` (índice) e Y (árvore de trabalho).
    pub index: char,
    pub worktree: char,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeStatus {
    pub dirty: bool,
    /// Commits na branch da tarefa que ainda não estão na base.
    pub ahead: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// `None` em arquivo binário.
    pub added: Option<u32>,
    pub deleted: Option<u32>,
    pub patch: String,
    pub truncated: bool,
}

async fn run(cwd: &Path, args: &[&str], env: &[(&str, OsString)], cap: usize, timeout: Duration) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.quotepath=off", "-c", "color.ui=never"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let work = async {
        let mut child = cmd.spawn()?;
        let mut stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");
        let err_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut buf).await;
            buf
        });
        let mut buf = Vec::new();
        (&mut stdout).take(cap as u64 + 1).read_to_end(&mut buf).await?;
        let truncated = buf.len() > cap;
        if truncated {
            buf.truncate(cap);
            let _ = child.kill().await;
        }
        let status = child.wait().await?;
        let stderr = String::from_utf8_lossy(&err_task.await.unwrap_or_default()).trim().to_string();
        if !truncated && !status.success() {
            return Err(GitError::Failed { args: args.join(" "), stderr });
        }
        let mut text = String::from_utf8_lossy(&buf).into_owned();
        if truncated {
            text.push_str("\n… (saída truncada)");
        }
        Ok(text)
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| GitError::Timeout(timeout.as_secs()))?
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    run(cwd, args, &[], OUT_CAP, TIMEOUT).await
}

/// Raiz do checkout que contém `cwd`, ou `None` se não for um repositório.
pub async fn toplevel(cwd: &Path) -> Option<PathBuf> {
    git(cwd, &["rev-parse", "--show-toplevel"]).await.ok().map(|s| PathBuf::from(s.trim()))
}

pub async fn status(cwd: &Path) -> Result<Vec<FileStatus>, GitError> {
    let out = git(cwd, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]).await?;
    let mut entries = out.split('\0').filter(|e| e.len() > 3);
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        let mut chars = entry.chars();
        let (index, worktree) = (chars.next().unwrap_or(' '), chars.next().unwrap_or(' '));
        files.push(FileStatus { index, worktree, path: entry[3..].to_string() });
        // Renomeação traz o caminho de origem como entrada seguinte.
        if index == 'R' || index == 'C' {
            entries.next();
        }
    }
    Ok(files)
}

/// Diff unificado (teto de 2 MiB). `staged` compara o índice com HEAD.
pub async fn diff(cwd: &Path, staged: bool) -> Result<String, GitError> {
    git(cwd, if staged { &["diff", "--cached"] } else { &["diff"] }).await
}

pub async fn stage(cwd: &Path, paths: &[String]) -> Result<(), GitError> {
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(cwd, &args).await.map(drop)
}

pub async fn unstage(cwd: &Path, paths: &[String]) -> Result<(), GitError> {
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(cwd, &args).await.map(drop)
}

pub async fn commit(cwd: &Path, message: &str) -> Result<(), GitError> {
    run(cwd, &["commit", "-m", message], &[], OUT_CAP, COMMIT_TIMEOUT).await.map(drop)
}

pub async fn branches(cwd: &Path) -> Result<Vec<String>, GitError> {
    let out = git(cwd, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]).await?;
    Ok(out.lines().map(str::to_string).collect())
}

pub async fn current_branch(cwd: &Path) -> Result<String, GitError> {
    Ok(git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await?.trim().to_string())
}

// ---------- worktrees ----------

async fn validate_branch(cwd: &Path, branch: &str) -> Result<(), GitError> {
    let bad = branch.is_empty() || branch.starts_with('-') || branch.contains("..");
    if bad || git(cwd, &["check-ref-format", "--branch", branch]).await.is_err() {
        return Err(GitError::InvalidBranch(branch.to_string()));
    }
    Ok(())
}

/// `<pasta-do-projeto>-worktrees/<branch com / trocado por ->`, ao lado do projeto.
pub fn worktree_path(repo: &Path, branch: &str) -> PathBuf {
    let name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
    repo.with_file_name(format!("{name}-worktrees")).join(branch.replace('/', "-"))
}

/// Cria a branch `branch` (a partir de `base` ou do HEAD) num worktree novo e devolve o caminho.
pub async fn create_worktree(repo: &Path, branch: &str, base: Option<&str>) -> Result<PathBuf, GitError> {
    validate_branch(repo, branch).await?;
    let repo = toplevel(repo).await.ok_or_else(|| GitError::Other("não é um repositório git".into()))?;
    let path = worktree_path(&repo, branch);
    if path.exists() {
        return Err(GitError::Other(format!("já existe: {}", path.display())));
    }
    let path_str = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add", "-b", branch, &path_str];
    args.extend(base);
    git(&repo, &args).await?;
    Ok(path)
}

pub async fn worktree_list(repo: &Path) -> Result<String, GitError> {
    git(repo, &["worktree", "list"]).await
}

/// Sujo (alterações não commitadas) e quantos commits à frente de `base`.
pub async fn worktree_status(path: &Path, base: &str) -> Result<WorktreeStatus, GitError> {
    let dirty = !status(path).await?.is_empty();
    let range = format!("{base}..HEAD");
    let ahead = git(path, &["rev-list", "--count", &range]).await?.trim().parse().unwrap_or(0);
    Ok(WorktreeStatus { dirty, ahead })
}

/// Remove o worktree sem `--force`: com alterações não commitadas o git recusa.
pub async fn remove_worktree(repo: &Path, path: &Path) -> Result<(), GitError> {
    git(repo, &["worktree", "remove", &path.to_string_lossy()]).await.map(drop)
}

/// Recria o checkout de uma branch existente cujo worktree sumiu do disco.
pub async fn recreate_worktree(repo: &Path, branch: &str) -> Result<PathBuf, GitError> {
    validate_branch(repo, branch).await?;
    git(repo, &["worktree", "prune"]).await?;
    let path = worktree_path(repo, branch);
    git(repo, &["worktree", "add", &path.to_string_lossy(), branch]).await?;
    Ok(path)
}

impl Database {
    /// Cria o worktree da tarefa e passa a usá-lo como `cwd` da thread (agente, fs e git resolvem nele).
    pub async fn attach_worktree(&self, thread_id: &str, repo: &Path, branch: &str) -> Result<PathBuf, GitError> {
        // Com um worktree já anexado, `repo` seria o próprio worktree e o novo ficaria aninhado.
        if let Some(existing) = self.get_thread(thread_id)?.worktree_path {
            return Err(GitError::Other(format!("a sessão já tem um worktree: {existing}")));
        }
        let path = create_worktree(repo, branch, None).await?;
        if let Err(e) = self.set_thread_worktree(thread_id, &path.to_string_lossy(), branch) {
            let _ = remove_worktree(repo, &path).await;
            return Err(e.into());
        }
        Ok(path)
    }
}

// ---------- mudanças por turno ----------

/// Foto do conteúdo do diretório como árvore git, sem tocar no índice nem nas refs do usuário:
/// usa um índice temporário (`GIT_INDEX_FILE`) semeado com HEAD e `add -A` (respeita .gitignore).
async fn in_index(root: &Path, env: &[(&str, OsString)], args: &[&str]) -> Result<String, GitError> {
    run(root, args, env, OUT_CAP, TIMEOUT).await
}

/// Apaga índices temporários que um snapshot interrompido (processo morto) deixou para trás.
/// O limite de 1 h poupa o de outro processo ainda em andamento.
fn sweep_stale_indexes(git_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(git_dir) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > Duration::from_secs(3600)));
        if old && entry.file_name().to_string_lossy().starts_with("rusi-index-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub async fn snapshot(cwd: &Path) -> Result<String, GitError> {
    let git_dir = git(cwd, &["rev-parse", "--absolute-git-dir"]).await?;
    sweep_stale_indexes(Path::new(git_dir.trim()));
    let root = toplevel(cwd).await.ok_or_else(|| GitError::Other("não é um repositório git".into()))?;
    let index = PathBuf::from(git_dir.trim()).join(format!("rusi-index-{}", uuid::Uuid::new_v4()));
    let env = [("GIT_INDEX_FILE", index.clone().into_os_string())];
    let result = async {
        // Parte do índice real (inclui o que o usuário já deixou staged); sem índice, começa vazio.
        let _ = std::fs::copy(PathBuf::from(git_dir.trim()).join("index"), &index);
        in_index(&root, &env, &["add", "-u"]).await?;
        // Os não rastreados entram por caminho, sem os grandes (shortcut: ficam fora das mudanças do turno).
        let listing = run(&root, &["ls-files", "-o", "--exclude-standard", "-z"], &[], 16 * OUT_CAP, TIMEOUT).await?;
        let small: Vec<&str> = listing
            .split('\0')
            .filter(|p| !p.is_empty())
            .filter(|p| std::fs::symlink_metadata(root.join(p)).is_ok_and(|m| m.len() <= SNAPSHOT_FILE_CAP))
            .collect();
        for chunk in small.chunks(200) {
            let mut args = vec!["add", "--"];
            args.extend(chunk);
            in_index(&root, &env, &args).await?;
        }
        Ok(in_index(&root, &env, &["write-tree"]).await?.trim().to_string())
    }
    .await;
    let _ = std::fs::remove_file(&index);
    result
}

/// Arquivos que mudaram entre duas fotos, com +/- e patch (256 KiB/arquivo, 200 arquivos).
pub async fn changes(cwd: &Path, before: &str, after: &str) -> Result<Vec<FileChange>, GitError> {
    if before == after {
        return Ok(Vec::new());
    }
    let numstat = git(cwd, &["diff", "--no-renames", "--numstat", "-z", before, after]).await?;
    let patch = git(cwd, &["diff", "--no-renames", before, after]).await?;
    // Mesma ordem nas duas saídas: o n-ésimo bloco `diff --git` é o n-ésimo arquivo do numstat.
    let mut patches = patch.split("\ndiff --git ").map(|p| p.trim_start_matches("diff --git "));
    let mut files = Vec::new();
    for entry in numstat.split('\0').filter(|e| !e.is_empty()).take(MAX_FILES) {
        let mut parts = entry.splitn(3, '\t');
        let (added, deleted, path) = (parts.next(), parts.next(), parts.next().unwrap_or_default());
        let mut text = patches.next().map(|p| format!("diff --git {p}")).unwrap_or_default();
        // Sem patch para um arquivo de texto: a saída total estourou o teto antes de chegar nele.
        let truncated = text.len() > FILE_PATCH_CAP || (text.is_empty() && added.is_some_and(|a| a != "-"));
        if truncated {
            let mut end = FILE_PATCH_CAP;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        files.push(FileChange {
            path: path.to_string(),
            added: added.and_then(|n| n.parse().ok()),
            deleted: deleted.and_then(|n| n.parse().ok()),
            patch: text,
            truncated,
        });
    }
    Ok(files)
}

/// Roda `turn` entre duas fotos e grava um evento `changes` se algo mudou.
/// Fora de um repositório (ou se a foto falhar) apenas executa o turno.
pub async fn track_changes<T>(
    db: &Database,
    thread_id: &str,
    cwd: Option<&Path>,
    turn: impl std::future::Future<Output = T>,
) -> T {
    let cwd = cwd.filter(|c| c.is_dir());
    let before = match cwd {
        Some(c) => snapshot(c).await.ok(),
        None => None,
    };
    let result = turn.await;
    if let (Some(cwd), Some(before)) = (cwd, before) {
        if let Ok(after) = snapshot(cwd).await {
            match changes(cwd, &before, &after).await {
                Ok(files) if !files.is_empty() => {
                    let _ = db.append_event(thread_id, "changes", &serde_json::json!({ "files": files }));
                }
                _ => {}
            }
        }
    }
    result
}
