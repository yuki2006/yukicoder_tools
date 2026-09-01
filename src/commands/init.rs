//! `yuki init` — リポジトリ直下に `yukicoder.toml` を作り、問題を取得する。

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::config::{Config, Repo, CONFIG_FILE, DOTENV_FILE};
use crate::local::{display_path, ProblemDir};

pub fn run(problem_id: i64, testcases: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("カレントディレクトリを取得できませんでした")?;
    let config_path = cwd.join(CONFIG_FILE);

    // 管理対象の一覧は持たない。問題ディレクトリの problem.toml が正なので、
    // ここで作るのはリポジトリの目印としての設定ファイルだけ。
    let repo = if config_path.is_file() {
        Repo::load(&cwd)?
    } else {
        let repo = Repo {
            root: cwd.clone(),
            config: Config::default(),
        };
        repo.save()?;
        println!("{} を書きました", display_path(&config_path));
        repo
    };

    warn_if_dotenv_tracked(&cwd);

    let client = crate::commands::Context { repo: repo.clone() }.client(problem_id)?;
    let dir = ProblemDir::new(repo.problem_dir(problem_id)?, problem_id);
    crate::commands::pull::pull_one(&client, &dir, testcases)
}

/// `.env` を置く場合は必ず `.gitignore` に入れる。トークンの流出を防ぐ。
fn warn_if_dotenv_tracked(root: &Path) {
    let gitignore = root.join(".gitignore");
    let ignored = std::fs::read_to_string(&gitignore)
        .map(|text| text.lines().any(|line| line.trim() == DOTENV_FILE))
        .unwrap_or(false);
    if !ignored {
        eprintln!(
            "警告: {} に {DOTENV_FILE} がありません。トークンを {DOTENV_FILE} に置くなら、\
             先に .gitignore へ追加してください。",
            display_path(&gitignore)
        );
    }
}
