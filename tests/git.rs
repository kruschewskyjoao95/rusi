//! Git, worktrees e mudanças por turno contra repositórios temporários.

use std::path::Path;
use std::process::Command;

use rusi::git::{self, GitError};
use rusi::persistence::{Database, NewThread};

fn sh(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Repositório em `<tmp>/proj` com um commit; devolve o tempdir para o worktree ficar ao lado.
fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proj");
    std::fs::create_dir(&path).unwrap();
    sh(&path, &["init", "-q", "-b", "main"]);
    // Identidade local: `git::commit` não passa `-c user.*` e a máquina de CI pode não ter uma.
    sh(&path, &["config", "user.name", "t"]);
    sh(&path, &["config", "user.email", "t@t"]);
    sh(&path, &["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("a.txt"), "um\n").unwrap();
    sh(&path, &["add", "."]);
    sh(&path, &["commit", "-q", "-m", "inicial"]);
    (dir, path)
}

#[tokio::test]
async fn test_status_diff_stage_commit() {
    let (_dir, repo) = repo();
    std::fs::write(repo.join("a.txt"), "um\ndois\n").unwrap();
    std::fs::write(repo.join("b b.txt"), "novo\n").unwrap();

    let files = git::status(&repo).await.unwrap();
    assert!(files.iter().any(|f| f.path == "a.txt" && f.worktree == 'M'));
    assert!(files.iter().any(|f| f.path == "b b.txt" && f.worktree == '?'));
    assert!(git::diff(&repo, false).await.unwrap().contains("+dois"));

    git::stage(&repo, &["a.txt".into()]).await.unwrap();
    assert!(git::diff(&repo, true).await.unwrap().contains("+dois"));
    git::commit(&repo, "segundo").await.unwrap();
    assert_eq!(git::status(&repo).await.unwrap().len(), 1);
    assert_eq!(git::branches(&repo).await.unwrap(), ["main"]);
}

#[tokio::test]
async fn test_worktree_lifecycle_and_branch_validation() {
    let (_dir, repo) = repo();
    for bad in ["", "-x", "a..b", "com espaço", "x.lock"] {
        let err = git::create_worktree(&repo, bad, None).await.unwrap_err();
        assert!(matches!(err, GitError::InvalidBranch(_)), "{bad:?}: {err}");
    }

    let path = git::create_worktree(&repo, "feat/um", None).await.unwrap();
    assert_eq!(path, git::worktree_path(&repo, "feat/um"));
    assert!(path.ends_with("proj-worktrees/feat-um") && path.join("a.txt").exists());
    assert_eq!(git::current_branch(&path).await.unwrap(), "feat/um");

    let clean = git::worktree_status(&path, "main").await.unwrap();
    assert_eq!((clean.dirty, clean.ahead), (false, 0));
    std::fs::write(path.join("c.txt"), "x\n").unwrap();
    sh(&path, &["add", "."]);
    sh(&path, &["commit", "-q", "-m", "c"]);
    std::fs::write(path.join("d.txt"), "y\n").unwrap();
    let busy = git::worktree_status(&path, "main").await.unwrap();
    assert_eq!((busy.dirty, busy.ahead), (true, 1));

    // Sem --force: com arquivo não rastreado o git recusa e o diretório continua lá.
    assert!(git::remove_worktree(&repo, &path).await.is_err());
    assert!(path.exists());
    std::fs::remove_file(path.join("d.txt")).unwrap();
    git::remove_worktree(&repo, &path).await.unwrap();
    assert!(!path.exists());

    // A branch sobrevive à remoção e o checkout pode ser recriado.
    let again = git::recreate_worktree(&repo, "feat/um").await.unwrap();
    assert!(again.join("c.txt").exists());
}

#[tokio::test]
async fn test_attach_worktree_moves_thread_cwd() {
    let (_dir, repo) = repo();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "claude".into(), cwd: Some(repo.display().to_string()), ..Default::default() }).unwrap();
    let path = db.attach_worktree(&thread.id, &repo, "tarefa").await.unwrap();
    let thread = db.get_thread(&thread.id).unwrap();
    assert_eq!(thread.cwd.as_deref(), Some(path.to_str().unwrap()));
    assert_eq!(thread.worktree_path, thread.cwd);
    assert_eq!(thread.branch.as_deref(), Some("tarefa"));

    // Um segundo worktree na mesma sessão é recusado e nada muda.
    let err = db.attach_worktree(&thread.id, &path, "outra").await.unwrap_err();
    assert!(err.to_string().contains("já tem um worktree"), "{err}");
    assert_eq!(db.get_thread(&thread.id).unwrap().branch.as_deref(), Some("tarefa"));
    assert!(!git::worktree_path(&path, "outra").exists());
}

#[tokio::test]
async fn test_turn_changes_do_not_touch_index_or_refs() {
    let (_dir, repo) = repo();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "claude".into(), ..Default::default() }).unwrap();
    // Algo já staged pelo usuário antes do turno deve continuar staged.
    std::fs::write(repo.join("staged.txt"), "s\n").unwrap();
    sh(&repo, &["add", "staged.txt"]);
    std::fs::write(repo.join(".gitignore"), "ignorado\n").unwrap();
    let head = git::current_branch(&repo).await.unwrap();

    git::track_changes(&db, &thread.id, Some(&repo), async {
        std::fs::write(repo.join("a.txt"), "um\ndois\n").unwrap();
        std::fs::write(repo.join("novo.txt"), "n\n").unwrap();
        std::fs::write(repo.join("ignorado"), "z\n").unwrap();
        std::fs::write(repo.join("staged.txt"), "s\nmais\n").unwrap();
        std::fs::write(repo.join("bin.dat"), [0u8, 159, 146, 150, 0]).unwrap();
    })
    .await;

    let events: Vec<_> = db.thread_events(&thread.id, 0).unwrap().into_iter().filter(|e| e.kind == "changes").collect();
    assert_eq!(events.len(), 1);
    let files = events[0].payload["files"].as_array().unwrap();
    let find = |p: &str| files.iter().find(|f| f["path"] == p).unwrap_or_else(|| panic!("sem {p}: {files:?}"));
    assert_eq!((find("a.txt")["added"].as_u64(), find("a.txt")["deleted"].as_u64()), (Some(1), Some(0)));
    assert!(find("a.txt")["patch"].as_str().unwrap().contains("+dois"));
    assert_eq!(find("novo.txt")["added"].as_u64(), Some(1));
    assert!(find("bin.dat")["added"].is_null());
    assert!(!files.iter().any(|f| f["path"] == "ignorado"));
    // staged.txt já existia (só no índice) e mudou durante o turno; .gitignore não mudou.
    assert_eq!(find("staged.txt")["added"].as_u64(), Some(1));
    assert!(!files.iter().any(|f| f["path"] == ".gitignore"));

    let status = git::status(&repo).await.unwrap();
    assert!(status.iter().any(|f| f.path == "staged.txt" && f.index == 'A'), "índice do usuário intacto: {status:?}");
    assert_eq!(git::current_branch(&repo).await.unwrap(), head);
    // O índice temporário não fica para trás.
    let leftovers = std::fs::read_dir(repo.join(".git")).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("rusi-index")).count();
    assert_eq!(leftovers, 0);
}

#[tokio::test]
async fn test_turn_without_changes_or_outside_repo_records_nothing() {
    let (dir, repo) = repo();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "claude".into(), ..Default::default() }).unwrap();

    let out = git::track_changes(&db, &thread.id, Some(&repo), async { 7 }).await;
    assert_eq!(out, 7);
    let plain = dir.path().join("fora");
    std::fs::create_dir(&plain).unwrap();
    git::track_changes(&db, &thread.id, Some(&plain), async { std::fs::write(plain.join("x"), "x").unwrap() }).await;
    git::track_changes(&db, &thread.id, None, async {}).await;
    assert!(db.thread_events(&thread.id, 0).unwrap().is_empty());
}

#[tokio::test]
async fn test_big_patch_is_truncated() {
    let (_dir, repo) = repo();
    let before = git::snapshot(&repo).await.unwrap();
    std::fs::write(repo.join("grande.txt"), "linha\n".repeat(100_000)).unwrap();
    let after = git::snapshot(&repo).await.unwrap();
    let files = git::changes(&repo, &before, &after).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].added, Some(100_000));
    assert!(files[0].truncated && files[0].patch.len() <= 256 * 1024);
}

#[tokio::test]
async fn test_files_after_an_oversized_patch_are_marked_truncated() {
    let (_dir, repo) = repo();
    // Rastreado: arquivos não rastreados grandes nem entram no snapshot.
    std::fs::write(repo.join("a_grande.txt"), "x\n").unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-q", "-m", "grande"]);
    let before = git::snapshot(&repo).await.unwrap();
    // O patch passa de 2 MiB e vem primeiro na ordem alfabética; o de z.txt não cabe na saída.
    std::fs::write(repo.join("a_grande.txt"), "linha\n".repeat(400_000)).unwrap();
    std::fs::write(repo.join("z.txt"), "pequeno\n").unwrap();
    let after = git::snapshot(&repo).await.unwrap();
    let files = git::changes(&repo, &before, &after).await.unwrap();
    let z = files.iter().find(|f| f.path == "z.txt").unwrap();
    assert!(z.patch.is_empty() && z.truncated);
}

#[tokio::test]
async fn test_snapshot_skips_big_untracked_files_and_finds_files_in_subdirs() {
    let (_dir, repo) = repo();
    let before = git::snapshot(&repo).await.unwrap();
    std::fs::create_dir(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/pequeno.txt"), "ok\n").unwrap();
    std::fs::write(repo.join("enorme.bin"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
    // Tirado de dentro da subpasta, o snapshot continua sendo do repositório inteiro.
    let after = git::snapshot(&repo.join("sub")).await.unwrap();
    let files = git::changes(&repo, &before, &after).await.unwrap();
    let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["sub/pequeno.txt"]);
}

#[tokio::test]
async fn test_snapshot_sweeps_only_stale_temporary_indexes() {
    let (_dir, repo) = repo();
    let old = repo.join(".git/rusi-index-velho");
    let fresh = repo.join(".git/rusi-index-recente");
    for f in [&old, &fresh] {
        std::fs::write(f, "x").unwrap();
    }
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    std::fs::File::options().write(true).open(&old).unwrap().set_modified(two_hours_ago).unwrap();
    git::snapshot(&repo).await.unwrap();
    assert!(!old.exists() && fresh.exists());
}
