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
    let keys = token_keys(problem_id);
    if let Some(token) = select_token(&keys, |key| std::env::var(key).ok(), &dotenv) {
        return Ok(token);
    }
    bail!(
        "トークンが見つかりません。次のいずれかを環境変数か {DOTENV_FILE} に設定してください: {}\n\
         GitHub Actions では Secrets を env に渡してください (例: YUKICODER_TOKEN: ${{{{ secrets.YUKICODER_TOKEN }}}})。",
        keys.join(", ")
    )
}

/// 探すキーを優先順に並べる。
///
/// 問題ごとのキーを先頭に置くので、`.env` に複数の問題の編集トークンを並べて
/// 書ける。編集トークンは 1 つの問題にしか使えないため、複数の問題を扱うなら
/// この形になる。
fn token_keys(problem_id: i64) -> Vec<String> {
    vec![
        format!("YUKICODER_TOKEN_{problem_id}"),
        "YUKICODER_TOKEN".to_string(),
        "YUKICODER_API_KEY".to_string(),
    ]
}

/// キーを順に見て、最初に見つかった空でない値を返す。
///
/// 同じキーなら環境変数が `.env` より優先される (CI の Secrets を効かせるため)。
fn select_token(
    keys: &[String],
    from_env: impl Fn(&str) -> Option<String>,
    dotenv: &HashMap<String, String>,
) -> Option<String> {
    for key in keys {
        for value in [from_env(key), dotenv.get(key).cloned()]
            .into_iter()
            .flatten()
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
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

    /// `.env` に複数の問題の編集トークンを並べられること。
    ///
    /// 編集トークンは 1 つの問題にしか使えないので、問題ごとのキーが
    /// 汎用のキーより先に効かないと、複数の問題を扱えない。
    #[test]
    fn per_problem_tokens_live_side_by_side() {
        let dotenv: HashMap<String, String> = [
            ("YUKICODER_TOKEN_13954", "ypt_for_13954"),
            ("YUKICODER_TOKEN_20000", "ypt_for_20000"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let no_env = |_: &str| None;

        assert_eq!(
            select_token(&token_keys(13954), no_env, &dotenv).unwrap(),
            "ypt_for_13954"
        );
        assert_eq!(
            select_token(&token_keys(20000), no_env, &dotenv).unwrap(),
            "ypt_for_20000"
        );
        // どちらのキーも無い問題は、汎用のキーが無ければ解決できない。
        assert!(select_token(&token_keys(99999), no_env, &dotenv).is_none());
    }

    /// 問題ごとのキーは汎用のキーより優先される。
    #[test]
    fn per_problem_token_wins_over_the_shared_one() {
        let dotenv: HashMap<String, String> = [
            ("YUKICODER_TOKEN_13954", "ypt_for_13954"),
            ("YUKICODER_TOKEN", "ypt_shared"),
            ("YUKICODER_API_KEY", "account_key"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let no_env = |_: &str| None;

        assert_eq!(
            select_token(&token_keys(13954), no_env, &dotenv).unwrap(),
            "ypt_for_13954"
        );
        // 個別のキーが無ければ汎用のキー、それも無ければ API キー。
        assert_eq!(
            select_token(&token_keys(20000), no_env, &dotenv).unwrap(),
            "ypt_shared"
        );
        let only_api_key: HashMap<String, String> =
            [("YUKICODER_API_KEY".to_string(), "account_key".to_string())]
                .into_iter()
                .collect();
        assert_eq!(
            select_token(&token_keys(20000), no_env, &only_api_key).unwrap(),
            "account_key"
        );
    }

    /// 同じキーなら環境変数が `.env` に勝つ。CI の Secrets を効かせるため。
    #[test]
    fn environment_wins_over_dotenv() {
        let dotenv: HashMap<String, String> = [(
            "YUKICODER_TOKEN_13954".to_string(),
            "ypt_from_dotenv".to_string(),
        )]
        .into_iter()
        .collect();
        let from_env =
            |key: &str| (key == "YUKICODER_TOKEN_13954").then(|| "ypt_from_env".to_string());

        assert_eq!(
            select_token(&token_keys(13954), from_env, &dotenv).unwrap(),
            "ypt_from_env"
        );
    }

    /// 空の値は「設定されていない」として次のキーへ進む。
    #[test]
    fn empty_values_are_skipped() {
        let dotenv: HashMap<String, String> = [
            ("YUKICODER_TOKEN_13954".to_string(), "   ".to_string()),
            ("YUKICODER_TOKEN".to_string(), "ypt_shared".to_string()),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            select_token(&token_keys(13954), |_| Some(String::new()), &dotenv).unwrap(),
            "ypt_shared"
        );
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
