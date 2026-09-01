//! `yuki new` — 問題のディレクトリ一式を作る。
//!
//! yukicoder で問題を作る (ID と編集トークンが発行される) と、サーバには
//! テンプレートの問題文が入っている。それを取得したうえで、`pull` は作らない
//! テストケースや想定解のディレクトリまで用意する。

use std::path::{Component, PathBuf};

use anyhow::{bail, Context as _, Result};

use crate::api::models::Which;
use crate::commands::Context;
use crate::local::{display_path, ProblemDir};

pub fn run(problem_id: i64, dir: Option<String>) -> Result<()> {
    let ctx = Context::discover()?;

    // 同じ問題が既にあると push が競合する。
    if let Some(existing) = ctx
        .repo
        .problems()?
        .into_iter()
        .find(|p| p.id == problem_id)
    {
        bail!(
            "問題 {problem_id} は {} にあります。取得し直すなら `yuki pull {problem_id}` です。",
            display_path(&existing.dir)
        );
    }

    let root = ctx
        .repo
        .root
        .join(&ctx.repo.config.problems_dir)
        .join(relative_dir(dir, problem_id)?);
    if root.exists() {
        bail!(
            "{} は既にあります。別の名前を --dir で指定してください。",
            display_path(&root)
        );
    }

    let client = ctx.client(problem_id)?;
    let dir = ProblemDir::new(root, problem_id);
    crate::commands::pull::pull_one(&client, &dir, true)?;

    // 空のディレクトリも作っておく。git は空ディレクトリを追跡しないが、
    // 書き始めるときに置き場所へ迷わないようにする。
    for path in [
        dir.testcase_dir(Which::In),
        dir.testcase_dir(Which::Out),
        dir.root().join("solutions"),
    ] {
        std::fs::create_dir_all(&path)
            .with_context(|| format!("{} を作成できませんでした", display_path(&path)))?;
    }

    println!(
        "\n{} に問題 {problem_id} を用意しました。\n\
         statement.md と problem.toml を編集して `yuki push {problem_id}` で反映します。",
        display_path(dir.root())
    );
    Ok(())
}

/// `--dir` を problems_dir からの相対パスとして検証する。
///
/// 外に出られると、リポジトリの外へ書いてしまう。
fn relative_dir(dir: Option<String>, problem_id: i64) -> Result<PathBuf> {
    let dir = dir.unwrap_or_else(|| problem_id.to_string());
    let path = PathBuf::from(&dir);
    let is_plain = path.components().all(|c| matches!(c, Component::Normal(_)));
    if dir.trim().is_empty() || !is_plain {
        bail!("--dir には problems ディレクトリからの相対パスを指定してください: {dir}");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::relative_dir;

    #[test]
    fn defaults_to_the_problem_id() {
        assert_eq!(
            relative_dir(None, 13954).unwrap(),
            std::path::Path::new("13954")
        );
    }

    #[test]
    fn nested_names_are_allowed() {
        assert_eq!(
            relative_dir(Some("abc001/a".into()), 1).unwrap(),
            std::path::Path::new("abc001/a")
        );
    }

    /// problems ディレクトリの外に書けてはいけない。
    #[test]
    fn escaping_paths_are_rejected() {
        for bad in ["../x", "/abs", "a/../../x", "", "  "] {
            assert!(relative_dir(Some(bad.into()), 1).is_err(), "{bad}");
        }
    }
}
