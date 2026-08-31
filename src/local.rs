//! ローカルの問題ディレクトリの読み書き。
//!
//! ```text
//! problems/<問題ID>/
//!   problem.toml            問題設定 (キー名は API と同じ)
//!   statement.md            問題文 (HTML で管理する問題は statement.html)
//!   editorial.md            解説 (HTML なら editorial.html)。無ければ触らない
//!   editorial_urls.txt      解説の外部URL一覧 (参照用。push では送らない)
//!   judge/
//!     judge.toml            スペシャルジャッジのジャッジコードの設定
//!     <sourceFile>          ジャッジコードのソース
//!   generator/
//!     generator.toml        ジェネレータの設定
//!     <sourceFile>          ジェネレータのソース
//!   testcases/
//!     in/*.txt
//!     out/*.txt
//!   solutions/              提出用のソース (同期対象ではない)
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::api::models::{ProblemSettings, Statement, Which};

pub const SETTINGS_FILE: &str = "problem.toml";
pub const GENERATOR_CONFIG_FILE: &str = "generator.toml";
pub const JUDGE_CONFIG_FILE: &str = "judge.toml";
pub const EDITORIAL_URLS_FILE: &str = "editorial_urls.txt";

/// `judge/judge.toml`。スペシャルジャッジのジャッジコードの設定。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeConfig {
    /// 言語 ID (`yuki languages` で確認できる)。
    pub lang_id: String,
    /// ソースファイル名 (このディレクトリからの相対)。
    pub source_file: String,
}

/// `generator/generator.toml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorConfig {
    /// 言語 ID (`yuki languages` で確認できる)。
    pub lang_id: String,
    /// ソースファイル名 (このディレクトリからの相対)。
    pub source_file: String,
    /// 生成ケース数。
    pub test_case_num: i64,
    /// 生成するケース名の接頭辞。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// 1 つの問題のローカルディレクトリ。
#[derive(Debug, Clone)]
pub struct ProblemDir {
    root: PathBuf,
    problem_id: i64,
}

impl ProblemDir {
    pub fn new(root: PathBuf, problem_id: i64) -> Self {
        Self { root, problem_id }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn problem_id(&self) -> i64 {
        self.problem_id
    }

    // ---- 問題設定 -------------------------------------------------------

    pub fn settings_path(&self) -> PathBuf {
        self.root.join(SETTINGS_FILE)
    }

    pub fn read_settings(&self) -> Result<ProblemSettings> {
        let path = self.settings_path();
        let text = read_text(&path)?;
        toml::from_str(&text)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))
    }

    pub fn write_settings(&self, settings: &ProblemSettings) -> Result<()> {
        let body =
            toml::to_string_pretty(settings).context("問題設定を TOML にできませんでした")?;
        let text = format!(
            "# yukicoder 問題設定 (PUT /api/v1/problems/{}/edit と同じキー名)\n\
             # 変更したら `yuki push` で反映する。\n\
             {body}",
            self.problem_id
        );
        write_text(&self.settings_path(), &text)
    }

    // ---- 問題文 ---------------------------------------------------------

    pub fn statement_path(&self, is_markdown: bool) -> PathBuf {
        self.root.join(if is_markdown {
            "statement.md"
        } else {
            "statement.html"
        })
    }

    /// 問題文を読む。`.md` と `.html` が両方あると、どちらを送るか決められない
    /// ので中止する。
    pub fn read_statement(&self) -> Result<Statement> {
        read_one_of(
            &self.statement_path(true),
            &self.statement_path(false),
            "問題文",
        )
    }

    pub fn write_statement(&self, statement: &Statement) -> Result<()> {
        write_text(
            &self.statement_path(statement.is_markdown()),
            statement.text(),
        )?;
        remove_stale(&self.statement_path(!statement.is_markdown()))
    }

    // ---- 解説 -----------------------------------------------------------

    pub fn editorial_path(&self, is_markdown: bool) -> PathBuf {
        self.root.join(if is_markdown {
            "editorial.md"
        } else {
            "editorial.html"
        })
    }

    pub fn editorial_urls_path(&self) -> PathBuf {
        self.root.join(EDITORIAL_URLS_FILE)
    }

    pub fn has_editorial(&self) -> bool {
        self.editorial_path(true).is_file() || self.editorial_path(false).is_file()
    }

    pub fn read_editorial(&self) -> Result<Statement> {
        read_one_of(
            &self.editorial_path(true),
            &self.editorial_path(false),
            "解説",
        )
    }

    pub fn write_editorial(&self, statement: &Statement) -> Result<()> {
        write_text(
            &self.editorial_path(statement.is_markdown()),
            statement.text(),
        )?;
        remove_stale(&self.editorial_path(!statement.is_markdown()))
    }

    // ---- ジェネレータ ---------------------------------------------------

    pub fn generator_dir(&self) -> PathBuf {
        self.root.join("generator")
    }

    pub fn generator_config_path(&self) -> PathBuf {
        self.generator_dir().join(GENERATOR_CONFIG_FILE)
    }

    pub fn has_generator(&self) -> bool {
        self.generator_config_path().is_file()
    }

    pub fn read_generator(&self) -> Result<(GeneratorConfig, String)> {
        let path = self.generator_config_path();
        let text = read_text(&path)?;
        let config: GeneratorConfig = toml::from_str(&text)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))?;
        let source = read_text(&self.generator_dir().join(&config.source_file))?;
        Ok((config, source))
    }

    pub fn write_generator(&self, config: &GeneratorConfig, source: &str) -> Result<()> {
        let text =
            toml::to_string_pretty(config).context("ジェネレータ設定を TOML にできませんでした")?;
        write_text(&self.generator_config_path(), &text)?;
        write_text(&self.generator_dir().join(&config.source_file), source)
    }

    // ---- ジャッジコード (スペシャルジャッジ) -----------------------------

    pub fn judge_dir(&self) -> PathBuf {
        self.root.join("judge")
    }

    pub fn judge_config_path(&self) -> PathBuf {
        self.judge_dir().join(JUDGE_CONFIG_FILE)
    }

    pub fn has_judge_code(&self) -> bool {
        self.judge_config_path().is_file()
    }

    pub fn read_judge_code(&self) -> Result<(JudgeConfig, String)> {
        let path = self.judge_config_path();
        let text = read_text(&path)?;
        let config: JudgeConfig = toml::from_str(&text)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))?;
        let source = read_text(&self.judge_dir().join(&config.source_file))?;
        Ok((config, source))
    }

    pub fn write_judge_code(&self, config: &JudgeConfig, source: &str) -> Result<()> {
        let text = toml::to_string_pretty(config)
            .context("ジャッジコードの設定を TOML にできませんでした")?;
        write_text(&self.judge_config_path(), &text)?;
        write_text(&self.judge_dir().join(&config.source_file), source)
    }

    // ---- テストケース ---------------------------------------------------

    pub fn testcase_dir(&self, which: Which) -> PathBuf {
        self.root.join("testcases").join(which.as_str())
    }

    /// ローカルのテストケースをファイル名順に読む。改行は LF に揃える。
    pub fn read_testcases(&self, which: Which) -> Result<BTreeMap<String, String>> {
        let dir = self.testcase_dir(which);
        let mut cases = BTreeMap::new();
        if !dir.is_dir() {
            return Ok(cases);
        }
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("{} を読めませんでした", display_path(&dir)))?;
        for entry in entries {
            let entry =
                entry.with_context(|| format!("{} を読めませんでした", display_path(&dir)))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                bail!("{} のファイル名を扱えません", display_path(&path));
            };
            if name.starts_with('.') {
                continue;
            }
            cases.insert(name.to_string(), read_text(&path)?);
        }
        Ok(cases)
    }

    pub fn write_testcase(&self, which: Which, name: &str, content: &str) -> Result<()> {
        write_text(&self.testcase_dir(which).join(name), content)
    }
}

/// `.md` と `.html` のどちらか片方だけを読む。
fn read_one_of(markdown: &Path, html: &Path, what: &str) -> Result<Statement> {
    match (markdown.is_file(), html.is_file()) {
        (true, true) => bail!(
            "{what}が {} と {} の両方にあります。どちらか一方だけにしてください。",
            display_path(markdown),
            display_path(html)
        ),
        (true, false) => Ok(Statement::Markdown(read_text(markdown)?)),
        (false, true) => Ok(Statement::Html(read_text(html)?)),
        (false, false) => bail!(
            "{what}のファイルがありません ({} か {})",
            display_path(markdown),
            display_path(html)
        ),
    }
}

/// 形式が切り替わったときに、古い方のファイルを残さない。
fn remove_stale(path: &Path) -> Result<()> {
    if path.is_file() {
        fs::remove_file(path)
            .with_context(|| format!("{} を削除できませんでした", display_path(path)))?;
    }
    Ok(())
}

/// テキストを読む。BOM を落とし、改行を LF に揃える。
///
/// Windows のチェックアウトで CRLF になったファイルをそのまま送ると、問題文も
/// テストケースも差分が出続けるため、読み込み時に正規化する。
pub fn read_text(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("{} を読めませんでした", display_path(path)))?;
    let text = String::from_utf8(bytes)
        .with_context(|| format!("{} が UTF-8 ではありません", display_path(path)))?;
    Ok(normalize(&text))
}

/// テキストを LF で書く。親ディレクトリが無ければ作る。
pub fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("{} を作成できませんでした", display_path(parent)))?;
    }
    fs::write(path, normalize(text).as_bytes())
        .with_context(|| format!("{} を書けませんでした", display_path(path)))
}

fn normalize(text: &str) -> String {
    text.trim_start_matches('\u{feff}').replace("\r\n", "\n")
}

/// 出力用にパスを短くする。
///
/// カレントディレクトリからの相対にして、実行環境の絶対パスをログに残さない。
/// 区切りは `/` に揃えるので、Windows と CI で同じ表示になる。
pub fn display_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let shortened = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf());
    shortened.display().to_string().replace('\\', "/")
}
