//! ローカルの問題ディレクトリの読み書き。
//!
//! ```text
//! problems/<問題ID>/
//!   problem.toml            問題設定 (キー名は API と同じ)
//!   statement.md            問題文 (HTML で管理する問題は statement.html)
//!   editorial.md            解説 (HTML なら editorial.html)。無ければ触らない
//!   judge/
//!     judge.toml            スペシャルジャッジのジャッジコードの設定
//!     <sourceFile>          ジャッジコードのソース
//!   validator/
//!     validator.toml        テストケースを検証する validator の設定
//!     <sourceFile>          validator のソース
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

use crate::api::models::{
    ProblemSettings, Statement, Which, EPS_MODE_LABELS, JUDGE_TYPE_LABELS, PROBLEM_TYPE_LABELS,
};

pub const SETTINGS_FILE: &str = "problem.toml";
pub const GENERATOR_CONFIG_FILE: &str = "generator.toml";
pub const JUDGE_CONFIG_FILE: &str = "judge.toml";
pub const VALIDATOR_CONFIG_FILE: &str = "validator.toml";

/// `judge/judge.toml`。スペシャルジャッジのジャッジコードの設定。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeConfig {
    /// 言語 ID (`yuki-tool languages` で確認できる)。
    pub lang_id: String,
    /// ソースファイル名 (このディレクトリからの相対)。
    pub source_file: String,
}

/// `validator/validator.toml`。テストケースを検証する validator の設定。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorConfig {
    /// 言語 ID (`yuki-tool languages` で確認できる)。
    pub lang_id: String,
    /// ソースファイル名 (このディレクトリからの相対)。
    pub source_file: String,
}

/// `generator/generator.toml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorConfig {
    /// 言語 ID (`yuki-tool languages` で確認できる)。
    pub lang_id: String,
    /// ソースファイル名 (このディレクトリからの相対)。
    pub source_file: String,
    /// 生成ケース数。
    pub test_case_num: i64,
    /// 生成するケース名の接頭辞。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// `problem.toml` の中身。
///
/// `problemId` がその問題の identity で、ディレクトリ名には依存しない。
/// 残りは API の設定そのもの。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemFile {
    /// 対応する問題 ID。古い形式のために省略も許す。
    #[serde(default)]
    pub problem_id: Option<i64>,
    /// false にすると `--all` と暗黙の単一選択の対象から外れる (ID を明示した
    /// 実行では動く)。トークンの失効中などに、CI の同期を問題ごとに止めるための
    /// ローカル専用キーで、API には送らない。
    #[serde(default = "default_sync")]
    pub sync: bool,
    #[serde(flatten)]
    pub settings: ProblemSettings,
}

fn default_sync() -> bool {
    true
}

/// `problem.toml` を読む。
pub fn read_problem_file(path: &Path) -> Result<ProblemFile> {
    let text = read_text(path)?;
    toml::from_str(&text).with_context(|| format!("{} を解釈できませんでした", display_path(path)))
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

    /// `problem.toml` を読む。`problemId` が食い違っていたら止める。
    pub fn read_settings(&self) -> Result<ProblemSettings> {
        let path = self.settings_path();
        let file = read_problem_file(&path)?;
        match file.problem_id {
            Some(id) if id != self.problem_id => bail!(
                "{} の problemId は {id} ですが、問題 {} として扱われています",
                display_path(&path),
                self.problem_id
            ),
            _ => Ok(file.settings),
        }
    }

    pub fn write_settings(&self, settings: &ProblemSettings) -> Result<()> {
        let body =
            toml::to_string_pretty(settings).context("問題設定を TOML にできませんでした")?;
        // 数値コードのままだと意味が分からないので、コメントで意味を添える。
        let body = annotate_line(&body, "problemType", &code_comment(PROBLEM_TYPE_LABELS));
        let body = annotate_line(&body, "judgeType", &code_comment(JUDGE_TYPE_LABELS));
        let body = annotate_line(
            &body,
            "epsMode",
            &EPS_MODE_LABELS
                .iter()
                .map(|(code, label)| format!("\"{code}\":{label}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        // sync = false はローカル専用の設定なので、pull で書き直しても消さない。
        let sync_line = if existing_sync(&self.settings_path())? {
            ""
        } else {
            "sync = false # 同期を止めている (--all の対象外)。再開するなら行ごと消す。\n"
        };
        let text = format!(
            "# yukicoder 問題設定。どの問題かは problemId で決まる (ディレクトリ名は自由)。\n\
             # 他のキーは PUT /api/v1/problems/{{id}}/edit と同じ名前で、変更したら\n\
             # `yuki-tool push` で反映する。\n\
             problemId = {}\n\
             {sync_line}{body}",
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

    /// 既存の `generator.toml` を読む。無ければ `None`。
    ///
    /// ファイルがあるのに読めない場合はエラーにする。壊れた設定を「無い」と
    /// 同一視すると、pull が黙って既定の内容で作り直してしまう。
    pub fn existing_generator_config(&self) -> Result<Option<GeneratorConfig>> {
        let path = self.generator_config_path();
        if !path.is_file() {
            return Ok(None);
        }
        let text = read_text(&path)?;
        toml::from_str(&text)
            .map(Some)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))
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

    /// 既存の `judge.toml` を読む。無ければ `None`、壊れていたらエラー。
    pub fn existing_judge_config(&self) -> Result<Option<JudgeConfig>> {
        let path = self.judge_config_path();
        if !path.is_file() {
            return Ok(None);
        }
        let text = read_text(&path)?;
        toml::from_str(&text)
            .map(Some)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))
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

    // ---- validator ------------------------------------------------------

    pub fn validator_dir(&self) -> PathBuf {
        self.root.join("validator")
    }

    pub fn validator_config_path(&self) -> PathBuf {
        self.validator_dir().join(VALIDATOR_CONFIG_FILE)
    }

    pub fn has_validator(&self) -> bool {
        self.validator_config_path().is_file()
    }

    /// 既存の `validator.toml` を読む。無ければ `None`、壊れていたらエラー。
    pub fn existing_validator_config(&self) -> Result<Option<ValidatorConfig>> {
        let path = self.validator_config_path();
        if !path.is_file() {
            return Ok(None);
        }
        let text = read_text(&path)?;
        toml::from_str(&text)
            .map(Some)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))
    }

    pub fn read_validator(&self) -> Result<(ValidatorConfig, String)> {
        let path = self.validator_config_path();
        let text = read_text(&path)?;
        let config: ValidatorConfig = toml::from_str(&text)
            .with_context(|| format!("{} を解釈できませんでした", display_path(&path)))?;
        let source = read_text(&self.validator_dir().join(&config.source_file))?;
        Ok((config, source))
    }

    pub fn write_validator(&self, config: &ValidatorConfig, source: &str) -> Result<()> {
        let text =
            toml::to_string_pretty(config).context("validator の設定を TOML にできませんでした")?;
        write_text(&self.validator_config_path(), &text)?;
        write_text(&self.validator_dir().join(&config.source_file), source)
    }

    // ---- テストケース ---------------------------------------------------

    pub fn testcase_dir(&self, which: Which) -> PathBuf {
        self.root.join("testcases").join(which.as_str())
    }

    /// ローカルのテストケースをファイル名順に、そのまま読む。
    ///
    /// 内容は変換しない。yukicoder は保存時に書き換えるが、その結果は
    /// `push` のあとに取り込む。
    pub fn read_testcases(&self, which: Which) -> Result<BTreeMap<String, Vec<u8>>> {
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
            // サーバはファイル名から A-Za-z0-9._ 以外を取り除く。名前が変わると
            // アップロードのたびに別ファイルとして増え、差分も消えないので、
            // 送る前に止めてリネームしてもらう。
            let sanitized = sanitized_file_name(name);
            if sanitized != name {
                bail!(
                    "テストケース名 {}/{name} は yukicoder では {} になります \
                     (使えるのは A-Za-z0-9._ だけ)。ファイル名を変更してください。",
                    which,
                    if sanitized.is_empty() {
                        "空の名前".to_string()
                    } else {
                        sanitized
                    }
                );
            }
            cases.insert(name.to_string(), read_bytes(&path)?);
        }
        Ok(cases)
    }

    pub fn testcase_path(&self, which: Which, name: &str) -> PathBuf {
        self.testcase_dir(which).join(name)
    }

    pub fn write_testcase(&self, which: Which, name: &str, content: &[u8]) -> Result<()> {
        write_bytes(&self.testcase_path(which, name), content)
    }
}

/// 既存の `problem.toml` の `sync` の値。ファイルが無ければ true (同期する)。
///
/// 書き直す直前に呼ぶので、設定全体はパースしない (`problemId` などが欠けた
/// 途中状態のファイルでも、`sync` だけは保持できるように)。
fn existing_sync(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(true);
    }
    let text = read_text(path)?;
    let value: toml::Value = toml::from_str(&text)
        .with_context(|| format!("{} を解釈できませんでした", display_path(path)))?;
    match value.get("sync") {
        None => Ok(true),
        Some(toml::Value::Boolean(sync)) => Ok(*sync),
        Some(other) => bail!(
            "{} の sync は true / false で指定してください: {other}",
            display_path(path)
        ),
    }
}

/// 数値コードの意味をコメントにする (例: `0:通常 1:スペシャル 2:リアクティブ`)。
fn code_comment(labels: &[(i64, &str)]) -> String {
    labels
        .iter()
        .map(|(code, label)| format!("{code}:{label}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 生成した TOML の `key = ...` の行に、意味を書いたコメントを添える。
///
/// 直前に自分で生成したテキストだけを対象にするので、行の形は決まっている。
fn annotate_line(body: &str, key: &str, comment: &str) -> String {
    body.lines()
        .map(|line| {
            if line.starts_with(&format!("{key} = ")) {
                format!("{line} # {comment}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
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

/// BOM を落とし、改行を LF に揃える。
///
/// サーバは保存時に `\r\n` と単独の `\r` をどちらも `\n` に変換する。同じ
/// 変換をしておかないと、ローカルと保存後の内容が食い違って毎回差分が出る。
fn normalize(text: &str) -> String {
    text.trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// テストケースをそのまま読む。
///
/// 内容は一切変換しない。yukicoder は保存時に改行コードや行末の空白を書き
/// 換えるが、その規則をクライアントに持たせるとサーバ側が変わったときに
/// 食い違う。**正規化はサーバに任せ、結果を取り込む。**
pub fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("{} を読めませんでした", display_path(path)))
}

/// バイト列をそのまま書く。親ディレクトリが無ければ作る。
pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("{} を作成できませんでした", display_path(parent)))?;
    }
    fs::write(path, bytes).with_context(|| format!("{} を書けませんでした", display_path(path)))
}

/// バイト列の SHA-256 (16 進小文字)。
///
/// テストケース一覧 (`?detail=1`) の `sha256` と比較して、差分判定を
/// ダウンロードなしで行うために使う。
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// サーバがファイル名から `A-Za-z0-9._` 以外を取り除いた結果を返す。
///
/// アップロードした名前がそのまま保存されるとは限らない。例えば
/// `case-01.txt` は `case01.txt` になる。
pub fn sanitized_file_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_')
        .collect()
}

/// 出力用にパスを短くする。
///
/// カレントディレクトリからの相対にして、実行環境の絶対パスをログに残さない。
/// 区切りは `/` に揃えるので、Windows と CI で同じ表示になる。
pub fn display_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let shortened = std::env::current_dir()
        .ok()
        .and_then(|cwd| relative_from(path, &cwd))
        .unwrap_or_else(|| path.to_path_buf());
    let text = shortened.display().to_string().replace('\\', "/");
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

/// `base` から `path` への相対パスを組む (`..` を許す)。
///
/// サブディレクトリから実行したとき、ルート側のパスを絶対パスではなく
/// `../..` 形式で表示するため。先頭 (ドライブなど) から食い違う場合は
/// `..` で辿れないので `None`。
fn relative_from(path: &Path, base: &Path) -> Option<PathBuf> {
    let path_parts: Vec<_> = path.components().collect();
    let base_parts: Vec<_> = base.components().collect();
    let mut common = 0;
    while common < path_parts.len()
        && common < base_parts.len()
        && path_parts[common] == base_parts[common]
    {
        common += 1;
    }
    if common == 0 {
        return None;
    }
    let mut result = PathBuf::new();
    for _ in common..base_parts.len() {
        result.push("..");
    }
    for part in &path_parts[common..] {
        result.push(part);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テキストファイルの読み込みでは改行を LF に揃える (Windows の
    /// チェックアウト対策)。テストケースはこの経路を通らず、そのまま送る。
    #[test]
    fn text_line_endings_become_lf() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(normalize("\u{feff}a"), "a", "BOM を落とす");
    }

    /// 生成した problem.toml では数値コードの行に意味のコメントが付く。
    #[test]
    fn coded_lines_are_annotated() {
        let body = "judgeType = 2\ntimeLimitMs = 2000\n";
        let body = annotate_line(body, "judgeType", &code_comment(JUDGE_TYPE_LABELS));
        assert_eq!(
            body,
            "judgeType = 2 # 0:通常 1:スペシャル 2:リアクティブ\ntimeLimitMs = 2000\n"
        );
        // コメント付きでも TOML として読める。
        let parsed: toml::Value = toml::from_str(&body).unwrap();
        assert_eq!(parsed["judgeType"].as_integer(), Some(2));
    }

    /// pull は problem.toml を丸ごと書き直すが、ローカル専用の sync = false は
    /// 消えてはいけない (消えると、止めたはずの同期が pull のあと再開してしまう)。
    #[test]
    fn sync_false_survives_settings_rewrite() {
        let root = std::env::temp_dir().join(format!("yuki-tool-sync-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let dir = ProblemDir::new(root.clone(), 13954);
        let settings = ProblemSettings {
            title: "t".into(),
            tags: String::new(),
            level: 1.0,
            time_limit_ms: 2000,
            memory_limit: 1024,
            eps_mode: "-".into(),
            eps: "0.0".into(),
            wip: true,
            recruiting_tester: false,
            problem_type: 0,
            judge_type: 0,
            show_ans: false,
            enable_pure_judge: false,
            force_single_server_judge: false,
            allowed_langs: vec![],
        };

        dir.write_settings(&settings).unwrap();
        let written = read_problem_file(&dir.settings_path()).unwrap();
        assert!(written.sync, "既定は同期する (sync 行は書かない)");

        fs::write(
            dir.settings_path(),
            "problemId = 13954\nsync = false\ntitle = \"t\"\n",
        )
        .unwrap();
        dir.write_settings(&settings).unwrap();
        let rewritten = read_problem_file(&dir.settings_path()).unwrap();
        assert!(!rewritten.sync, "書き直しても sync = false は保持する");

        fs::write(dir.settings_path(), "sync = 1\n").unwrap();
        assert!(
            dir.write_settings(&settings).is_err(),
            "bool 以外の sync は黙って true 扱いにしない"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// sha256 はサーバの一覧 (`?detail=1`) の値と突き合わせるので、16 進
    /// 小文字で一致すること。
    #[test]
    fn sha256_matches_the_standard_encoding() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"1 2 3\n"),
            "1def07dbe06eeb097aafec8a40329937cd20c93a83634b8221ea2b41a894310c"
        );
    }

    /// サブディレクトリから実行しても、ルート側のパスを絶対パスにしない。
    #[test]
    fn paths_outside_the_cwd_become_dotdot_relative() {
        let base = Path::new("C:\\repo\\problems\\13954");
        assert_eq!(
            relative_from(Path::new("C:\\repo\\problems\\13954\\statement.md"), base).unwrap(),
            Path::new("statement.md")
        );
        assert_eq!(
            relative_from(Path::new("C:\\repo\\problems\\20000"), base).unwrap(),
            Path::new("..\\20000")
        );
        assert_eq!(
            relative_from(Path::new("C:\\repo"), base).unwrap(),
            Path::new("..\\..")
        );
        assert!(
            relative_from(Path::new("D:\\other"), base).is_none(),
            "別ドライブは .. で辿れない"
        );
    }

    /// ファイル名は A-Za-z0-9._ 以外が落ちる。
    #[test]
    fn file_names_lose_unsupported_characters() {
        assert_eq!(sanitized_file_name("1_sample_1.txt"), "1_sample_1.txt");
        assert_eq!(sanitized_file_name("case-01.txt"), "case01.txt");
        assert_eq!(sanitized_file_name("テスト 1.txt"), "1.txt");
    }
}
