//! リポジトリ設定 (`yukicoder.toml`) と、トークンの解決。
//!
//! トークンはコマンドライン引数では受け取らない。CI のログやプロセス一覧に
//! 残らないよう、環境変数か `.env` からだけ読む。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::api::DEFAULT_BASE_URL;
use crate::local::display_path;

pub const CONFIG_FILE: &str = "yukicoder.toml";
pub const DOTENV_FILE: &str = ".env";

/// リポジトリ直下の `yukicoder.toml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 管理対象の問題 ID。
    #[serde(default)]
    pub problems: Vec<i64>,
    /// 問題ディレクトリを置く場所。既定は `problems`。
    #[serde(default = "default_problems_dir")]
    pub problems_dir: String,
    /// API のベース URL。既定は `https://yukicoder.me/api`。
    #[serde(default = "default_base_url")]
    pub base_url: String,
}

fn default_problems_dir() -> String {
    "problems".to_string()
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            problems: Vec::new(),
            problems_dir: default_problems_dir(),
            base_url: default_base_url(),
        }
    }
}

/// 設定ファイルと、それが置かれているディレクトリ。
#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
    pub config: Config,
}

impl Repo {
    /// カレントディレクトリから上に向かって `yukicoder.toml` を探す。
    pub fn discover() -> Result<Self> {
        let cwd = std::env::current_dir().context("カレントディレクトリを取得できませんでした")?;
        let mut dir = cwd.as_path();
        loop {
            let candidate = dir.join(CONFIG_FILE);
            if candidate.is_file() {
                return Self::load(dir);
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => bail!(
                    "{CONFIG_FILE} が見つかりません。`yuki init <問題ID>` で作成してください。"
                ),
            }
        }
    }

    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(CONFIG_FILE);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("{} を読めませんでした", display_path(&path)))?;
        let config: Config = toml::from_str(&text)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))?;
        Ok(Self {
            root: root.to_path_buf(),
            config,
        })
    }

    pub fn save(&self) -> Result<()> {
        let path = self.root.join(CONFIG_FILE);
        let text =
            toml::to_string_pretty(&self.config).context("設定を TOML に変換できませんでした")?;
        crate::local::write_text(&path, &text)
    }

    /// 問題ディレクトリ (`<problems_dir>/<問題ID>`)。
    pub fn problem_dir(&self, problem_id: i64) -> PathBuf {
        self.root
            .join(&self.config.problems_dir)
            .join(problem_id.to_string())
    }

    /// コマンドで対象にする問題 ID を決める。
    ///
    /// - `--all` なら設定ファイルのすべて
    /// - ID 指定があればそれ
    /// - どちらも無く、設定に問題が 1 つだけならそれ
    pub fn target_problems(&self, explicit: Option<i64>, all: bool) -> Result<Vec<i64>> {
        if all {
            if self.config.problems.is_empty() {
                bail!("{CONFIG_FILE} の problems が空です");
            }
            return Ok(self.config.problems.clone());
        }
        if let Some(id) = explicit {
            return Ok(vec![id]);
        }
        match self.config.problems.as_slice() {
            [only] => Ok(vec![*only]),
            [] => bail!("対象の問題 ID を指定してください ({CONFIG_FILE} の problems が空です)"),
            _ => bail!(
                "対象の問題 ID を指定してください (--all ですべて、または問題 ID を 1 つ指定)"
            ),
        }
    }
}

/// `.env` を最小限の形式で読む。
///
/// - `KEY=VALUE` の行だけを見る
/// - `#` で始まる行と空行は無視する
/// - 値の前後の引用符 (`"` / `'`) は取り除く
/// - すでにプロセスの環境変数にあるキーは上書きしない (CI の secrets が優先)
pub fn load_dotenv(path: &Path) -> Result<HashMap<String, String>> {
    let mut vars = HashMap::new();
    if !path.is_file() {
        return Ok(vars);
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("{} を読めませんでした", display_path(path)))?;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            bail!(
                "{}:{} は KEY=VALUE の形式ではありません",
                display_path(path),
                i + 1
            );
        };
        let key = key.trim().to_string();
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value)
            .to_string();
        vars.insert(key, value);
    }
    Ok(vars)
}

/// 環境変数と `.env` からトークンを解決する。
///
/// 探す順番:
/// 1. `YUKICODER_TOKEN_<問題ID>` (問題ごとの編集トークン)
/// 2. `YUKICODER_TOKEN`
/// 3. `YUKICODER_API_KEY` (アカウントの API キー)
///
/// それぞれ環境変数を先に見て、無ければ `.env` を見る。既定値は持たない。
pub fn resolve_token(repo_root: &Path, problem_id: i64) -> Result<String> {
    let dotenv = load_dotenv(&repo_root.join(DOTENV_FILE))?;
    let keys = [
        format!("YUKICODER_TOKEN_{problem_id}"),
        "YUKICODER_TOKEN".to_string(),
        "YUKICODER_API_KEY".to_string(),
    ];
    for key in &keys {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return Ok(value.trim().to_string());
            }
        }
        if let Some(value) = dotenv.get(key) {
            if !value.trim().is_empty() {
                return Ok(value.trim().to_string());
            }
        }
    }
    bail!(
        "トークンが見つかりません。次のいずれかを環境変数か {DOTENV_FILE} に設定してください: {}\n\
         GitHub Actions では Secrets を env に渡してください (例: YUKICODER_TOKEN: ${{{{ secrets.YUKICODER_TOKEN }}}})。",
        keys.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("yukicoder-tools-test-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dotenv_reads_key_values() {
        let dir = temp_dir("dotenv");
        let path = write(
            &dir,
            ".env.sample",
            "# コメント\n\nYUKICODER_TOKEN=ypt_abc\nexport YUKICODER_TOKEN_13954=\"ypt_def\"\nOTHER='x y'\n",
        );
        let vars = load_dotenv(&path).unwrap();
        assert_eq!(vars.get("YUKICODER_TOKEN").unwrap(), "ypt_abc");
        assert_eq!(vars.get("YUKICODER_TOKEN_13954").unwrap(), "ypt_def");
        assert_eq!(vars.get("OTHER").unwrap(), "x y");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn dotenv_missing_file_is_empty() {
        assert!(load_dotenv(Path::new("does-not-exist.env"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn dotenv_rejects_broken_line() {
        let dir = temp_dir("broken");
        let path = write(&dir, ".env.broken", "YUKICODER_TOKEN\n");
        assert!(load_dotenv(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn target_problems_needs_explicit_id_when_many() {
        let repo = Repo {
            root: PathBuf::from("."),
            config: Config {
                problems: vec![1, 2],
                ..Config::default()
            },
        };
        assert!(repo.target_problems(None, false).is_err());
        assert_eq!(repo.target_problems(Some(2), false).unwrap(), vec![2]);
        assert_eq!(repo.target_problems(None, true).unwrap(), vec![1, 2]);
    }

    #[test]
    fn target_problems_uses_the_only_problem() {
        let repo = Repo {
            root: PathBuf::from("."),
            config: Config {
                problems: vec![13954],
                ..Config::default()
            },
        };
        assert_eq!(repo.target_problems(None, false).unwrap(), vec![13954]);
    }
}
