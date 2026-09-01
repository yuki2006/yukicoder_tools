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
///
/// 管理対象の問題は、ここには書かない。`problems_dir` 以下の `problem.toml` を
/// 探し、その `problemId` で決まる。一覧を二重に持つと、ディレクトリを増やした
/// ときに片方だけ更新して食い違う。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 問題ディレクトリを置く場所。既定は `problems`。
    ///
    /// 全キーに既定値があるので、キーを打ち間違えると黙って既定値が使われて
    /// しまう。それを防ぐため未知のキーはエラーにする (`deny_unknown_fields`)。
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
            problems_dir: default_problems_dir(),
            base_url: default_base_url(),
        }
    }
}

/// リポジトリの中で見つかった 1 つの問題。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub id: i64,
    pub dir: PathBuf,
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

    /// リポジトリの中の問題をすべて見つける。
    ///
    /// `problems_dir` 以下を降りて `problem.toml` を探し、その `problemId` を読む。
    /// ディレクトリ名は問題 ID でなくてよい (`contests/abc001/a` のような置き方が
    /// できる)。`problemId` が無い古い形式は、ディレクトリ名が数値ならそれを使う。
    pub fn problems(&self) -> Result<Vec<Problem>> {
        let base = self.root.join(&self.config.problems_dir);
        let mut found = Vec::new();
        collect_problems(&base, &mut found)?;
        found.sort_by_key(|p| p.id);
        for pair in found.windows(2) {
            if pair[0].id == pair[1].id {
                bail!(
                    "問題 {} が {} と {} の両方にあります",
                    pair[0].id,
                    crate::local::display_path(&pair[0].dir),
                    crate::local::display_path(&pair[1].dir)
                );
            }
        }
        Ok(found)
    }

    /// 問題 ID からディレクトリを決める。
    ///
    /// 見つからなければ `<problems_dir>/<問題ID>` を使う (`init` や、まだ
    /// リポジトリに無い問題の `pull` 用)。
    pub fn problem_dir(&self, problem_id: i64) -> Result<PathBuf> {
        if let Some(problem) = self.problems()?.into_iter().find(|p| p.id == problem_id) {
            return Ok(problem.dir);
        }
        let fallback = self
            .root
            .join(&self.config.problems_dir)
            .join(problem_id.to_string());
        // ここまで来たのは、この ID の問題がリポジトリに無いから。それでも
        // 既定の場所に problem.toml があるなら、そこは別の問題のディレクトリで、
        // 書き込むと壊してしまう (problems/ は gitignore 済みで復元もできない)。
        if fallback.join(crate::local::SETTINGS_FILE).is_file() {
            bail!(
                "{} は別の問題のディレクトリです (problem.toml の problemId が {problem_id} \
                 ではありません)。",
                display_path(&fallback)
            );
        }
        Ok(fallback)
    }

    /// コマンドで対象にする問題 ID を決める。
    ///
    /// - `--all` ならリポジトリにあるすべて
    /// - ID 指定があればそれ
    /// - どちらも無く、リポジトリに問題が 1 つだけならそれ
    pub fn target_problems(&self, explicit: Option<i64>, all: bool) -> Result<Vec<i64>> {
        if let Some(id) = explicit {
            if all {
                bail!("--all と問題 ID は同時に指定できません");
            }
            return Ok(vec![id]);
        }
        let problems = self.problems()?;
        if all {
            if problems.is_empty() {
                bail!(
                    "{} に問題がありません",
                    crate::local::display_path(self.root.join(&self.config.problems_dir))
                );
            }
            return Ok(problems.into_iter().map(|p| p.id).collect());
        }
        match problems.as_slice() {
            [only] => Ok(vec![only.id]),
            [] => bail!("対象の問題 ID を指定してください (`yuki init <問題ID>` で取得できます)"),
            _ => bail!(
                "対象の問題 ID を指定してください (--all ですべて、または問題 ID を 1 つ指定)"
            ),
        }
    }
}

/// `problem.toml` のあるディレクトリを探す。見つけたらその下は見ない。
fn collect_problems(dir: &Path, found: &mut Vec<Problem>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let settings = dir.join(crate::local::SETTINGS_FILE);
    if settings.is_file() {
        let file = crate::local::read_problem_file(&settings)?;
        let id = match file.problem_id {
            Some(id) => id,
            // 古い形式。ディレクトリ名が問題 ID だったころのもの。
            None => dir
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<i64>().ok())
                .with_context(|| {
                    format!(
                        "{} に problemId がありません。`yuki pull <問題ID>` で書き足せます。",
                        crate::local::display_path(&settings)
                    )
                })?,
        };
        found.push(Problem {
            id,
            dir: dir.to_path_buf(),
        });
        return Ok(());
    }
    let entries = fs::read_dir(dir)
        .with_context(|| format!("{} を読めませんでした", crate::local::display_path(dir)))?;
    for entry in entries {
        let entry = entry
            .with_context(|| format!("{} を読めませんでした", crate::local::display_path(dir)))?;
        let path = entry.path();
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if path.is_dir() && !hidden {
            collect_problems(&path, found)?;
        }
    }
    Ok(())
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
    let keys = token_keys(problem_id);
    // .env が壊れていても、環境変数でトークンが決まるなら止めない。
    // 環境変数が優先という順序を、.env のパースエラーで覆さないため。
    let (dotenv, dotenv_error) = match load_dotenv(&repo_root.join(DOTENV_FILE)) {
        Ok(vars) => (vars, None),
        Err(err) => (HashMap::new(), Some(err)),
    };
    if let Some(token) = select_token(&keys, |key| std::env::var(key).ok(), &dotenv) {
        if let Some(err) = dotenv_error {
            eprintln!("警告: {err:#}。環境変数のトークンを使います。");
        }
        return Ok(token);
    }
    if let Some(err) = dotenv_error {
        return Err(err);
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

    /// `problem.toml` の最小構成。既定値のあるキーは省く。
    fn problem_toml(problem_id: Option<i64>) -> String {
        let id = match problem_id {
            Some(id) => format!("problemId = {id}\n"),
            None => String::new(),
        };
        format!(
            "{id}title = \"t\"\nlevel = 1.0\ntimeLimitMs = 2000\nmemoryLimit = 1024\n\
             epsMode = \"-\"\neps = \"0.0\"\nwip = true\nrecruitingTester = false\n\
             problemType = 0\njudgeType = 0\n"
        )
    }

    fn repo_with(problems: &[(&str, Option<i64>)], tag: &str) -> Repo {
        let root = temp_dir(tag);
        let _ = fs::remove_dir_all(&root);
        for (dir, id) in problems {
            let dir = root.join("problems").join(dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("problem.toml"), problem_toml(*id)).unwrap();
        }
        fs::create_dir_all(root.join("problems")).unwrap();
        Repo {
            root,
            config: Config::default(),
        }
    }

    /// ディレクトリ名は自由で、problemId が問題を決める。
    #[test]
    fn problems_are_found_by_the_id_in_the_file() {
        let repo = repo_with(
            &[("tutorial-dp", Some(13954)), ("abc001/a", Some(20000))],
            "discover",
        );
        let problems = repo.problems().unwrap();

        assert_eq!(
            problems.iter().map(|p| p.id).collect::<Vec<_>>(),
            [13954, 20000]
        );
        assert!(problems[0].dir.ends_with("tutorial-dp"));
        assert!(problems[1].dir.ends_with("a"));
        assert_eq!(repo.problem_dir(13954).unwrap(), problems[0].dir);
    }

    /// problemId が無い古い形式は、ディレクトリ名が問題 ID だとみなす。
    #[test]
    fn directory_name_is_the_fallback_id() {
        let repo = repo_with(&[("13954", None)], "fallback");
        assert_eq!(repo.problems().unwrap()[0].id, 13954);
    }

    /// problemId もディレクトリ名も手がかりにならなければ止める。
    #[test]
    fn missing_problem_id_is_an_error() {
        let repo = repo_with(&[("no-id-here", None)], "missing-id");
        assert!(repo.problems().is_err());
    }

    /// 同じ問題を 2 か所で管理すると、push が競合するので止める。
    #[test]
    fn duplicate_problem_ids_are_rejected() {
        let repo = repo_with(&[("a", Some(13954)), ("b", Some(13954))], "duplicate");
        assert!(repo.problems().is_err());
    }

    #[test]
    fn target_problems_needs_explicit_id_when_many() {
        let repo = repo_with(&[("a", Some(1)), ("b", Some(2))], "many");
        assert!(repo.target_problems(None, false).is_err());
        assert_eq!(repo.target_problems(Some(2), false).unwrap(), vec![2]);
        assert_eq!(repo.target_problems(None, true).unwrap(), vec![1, 2]);
        assert!(
            repo.target_problems(Some(2), true).is_err(),
            "--all と ID の同時指定はどちらを意図したか分からない"
        );
    }

    #[test]
    fn target_problems_uses_the_only_problem() {
        let repo = repo_with(&[("only", Some(13954))], "single");
        assert_eq!(repo.target_problems(None, false).unwrap(), vec![13954]);
    }

    /// リポジトリにまだ無い問題は、既定の場所を使う (init / pull 用)。
    #[test]
    fn unknown_problems_fall_back_to_the_default_directory() {
        let repo = repo_with(&[], "unknown");
        assert!(repo.problem_dir(13954).unwrap().ends_with("problems/13954"));
        assert!(repo.target_problems(None, false).is_err());
    }

    /// フォールバック先が別の問題のディレクトリなら、書き込ませない。
    /// pull がそこへ書くと、別の問題のローカルコピーを上書きしてしまう。
    #[test]
    fn fallback_owned_by_another_problem_is_rejected() {
        // problems/100 というディレクトリ名だが、中身は問題 200。
        let repo = repo_with(&[("100", Some(200))], "claimed");
        assert!(repo.problem_dir(100).is_err());
        assert!(repo.problem_dir(200).unwrap().ends_with("100"));
    }

    /// キーの打ち間違いを黙って既定値にしない。
    #[test]
    fn unknown_config_keys_are_rejected() {
        assert!(toml::from_str::<Config>("problem_dir = \"contests\"").is_err());
        assert!(toml::from_str::<Config>("problems_dir = \"contests\"").is_ok());
    }
}
