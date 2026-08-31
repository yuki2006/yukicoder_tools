//! `https://yukicoder.me/api/swagger.yaml` に対応するデータ型。
//!
//! フィールド名は API と同じ camelCase を使う。`problem.toml` などのローカル
//! ファイルも同じ名前で保存し、API のドキュメントをそのまま参照できるようにする。

use serde::{Deserialize, Serialize};

/// `GET /v1/problems/{id}/edit` のレスポンス。
///
/// `problemId` / `content` / `isMarkdown` / `showable` は読み取り専用で、
/// 書き込みには送らない。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemEditContent {
    pub problem_id: i64,
    /// 問題文の生テキスト。`is_markdown` が true なら Markdown、false なら HTML。
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub is_markdown: bool,
    /// 公開済みか。API からは変更できない。
    #[serde(default)]
    pub showable: bool,
    #[serde(flatten)]
    pub settings: ProblemSettings,
}

/// 問題の編集可能な設定。`problem.toml` に保存する内容そのもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemSettings {
    pub title: String,
    /// スペース区切り。
    #[serde(default)]
    pub tags: String,
    pub level: f64,
    /// ミリ秒。
    pub time_limit_ms: i64,
    /// MB。
    pub memory_limit: i64,
    /// `-` / `abs` / `rel` / `all`
    pub eps_mode: String,
    /// 許容誤差。API は文字列でも数値でも返すので文字列として保持する。
    #[serde(deserialize_with = "de::string_or_number")]
    pub eps: String,
    pub wip: bool,
    pub recruiting_tester: bool,
    /// 0:通常 1:教育的 2:スコア 3:ネタ 4:未証明 5:数学 6:ショートコード
    pub problem_type: i64,
    /// 0:通常 1:スペシャル 2:リアクティブ
    pub judge_type: i64,
    /// テスト後解答表示。
    #[serde(default)]
    pub show_ans: bool,
    #[serde(default)]
    pub enable_pure_judge: bool,
    #[serde(default)]
    pub force_single_server_judge: bool,
    /// 許可言語。空配列で全言語。
    #[serde(default)]
    pub allowed_langs: Vec<String>,
}

/// 問題文・解説の本文。Markdown と HTML は排他で、どちらか一方だけを送る。
///
/// サーバ側は `html` が非空ならそれをそのまま保存し、`markdown` は変換しない。
/// 両方を送ると「表示は html・ソースは markdown」という食い違った状態になる。
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Markdown(String),
    Html(String),
}

impl Statement {
    pub fn new(text: impl Into<String>, is_markdown: bool) -> Self {
        if is_markdown {
            Statement::Markdown(text.into())
        } else {
            Statement::Html(text.into())
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Statement::Markdown(s) | Statement::Html(s) => s,
        }
    }

    pub fn is_markdown(&self) -> bool {
        matches!(self, Statement::Markdown(_))
    }

    /// `(html, markdown)` に振り分ける。
    ///
    /// - Markdown: `html` は空文字列にする。サーバが Markdown を変換して保存する。
    /// - HTML: `markdown` は送らない。古い Markdown ソースを残さないため。
    fn into_fields(self) -> (String, Option<String>) {
        match self {
            Statement::Markdown(md) => (String::new(), Some(md)),
            Statement::Html(html) => (html, None),
        }
    }
}

/// `PUT /v1/problems/{id}/edit` のリクエスト。
///
/// 部分更新ではないので、設定は毎回すべて送る。未知のフィールドはエラーになる
/// ため、読み取り専用キーは含めない。`latestModNanoTime` は WebUI の競合検出
/// 専用なので送らない (last-write-wins)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemEditRequest {
    #[serde(flatten)]
    pub settings: ProblemSettings,
    /// 問題文(HTML)。Markdown で保存するときは空文字列。
    pub html: String,
    /// 問題文(Markdown)。渡すと Markdown として保存される。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

impl ProblemEditRequest {
    /// 問題文が空のまま送ると保存済みの問題文が消えるので、ここで止める。
    pub fn new(settings: ProblemSettings, statement: Statement) -> anyhow::Result<Self> {
        if statement.text().trim().is_empty() {
            anyhow::bail!("問題文が空です。空のまま送ると保存済みの問題文が消えるので中止します。");
        }
        let (html, markdown) = statement.into_fields();
        Ok(Self {
            settings,
            html,
            markdown,
        })
    }
}

/// 保存系 API のレスポンス。
///
/// `LatestModNanoTime` も返るが、WebUI の競合検出専用なので読まない。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SaveResponse {
    #[serde(default, rename = "Message")]
    pub message: String,
}

/// `GET /v1/problems/{id}/generator` のレスポンス。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorContent {
    /// 未登録なら空。
    #[serde(default)]
    pub lang_id: String,
    #[serde(default)]
    pub source: String,
    /// 生成の有効フラグ。管理者が承認すると true になる読み取り専用の値。
    #[serde(default)]
    pub enable: bool,
    #[serde(default)]
    pub test_case_num: i64,
}

/// `PUT /v1/problems/{id}/generator` のリクエスト。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorRequest {
    pub lang_id: String,
    /// 空文字列で削除。
    pub source: String,
    pub test_case_num: i64,
    /// true なら保存後にケース生成を起動する (1〜50 ケース)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate: Option<bool>,
    /// 生成するケース名の接頭辞。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// `GET /v1/problems/{id}/code` のレスポンス。
///
/// スペシャルジャッジのジャッジコード。未登録なら 3 つとも空。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeCodeContent {
    #[serde(default)]
    pub lang_id: String,
    #[serde(default)]
    pub source: String,
    /// コンパイル状態。保存直後は `WJ`、その後 `AC` か `CE` になる。
    #[serde(default)]
    pub status: String,
}

/// `PUT /v1/problems/{id}/code` のリクエスト。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeCodeRequest {
    pub lang_id: String,
    /// 空文字列で削除。
    pub source: String,
}

/// `PUT /v1/problems/{id}/code` のレスポンス。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JudgeCodeSaveResponse {
    #[serde(default, rename = "Message")]
    pub message: String,
    /// 保存直後は `WJ`。削除したときは空。
    #[serde(default)]
    pub status: String,
}

/// ジャッジコードのコンパイルが通った状態。
pub const JUDGE_STATUS_OK: &str = "AC";
/// ジャッジコードのコンパイルに失敗した状態。
pub const JUDGE_STATUS_COMPILE_ERROR: &str = "CE";

/// コンパイル状態が確定しているか。
///
/// 確定するのは `AC` と `CE` だけ。`WJ` (待機) と `Judge` (コンパイル中) は
/// 途中の状態で、空文字列は未登録。**確定していない値を失敗として扱わないこと。**
/// `WJ` → `Judge` → `AC` と遷移する。
pub fn judge_status_is_final(status: &str) -> bool {
    status == JUDGE_STATUS_OK || status == JUDGE_STATUS_COMPILE_ERROR
}

/// `GET /v1/problems/{id}/editorial` のレスポンス。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorialContent {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub is_markdown: bool,
}

/// `PUT /v1/problems/{id}/editorial` のリクエスト。
///
/// 問題文と同じく `html` / `markdown` は排他。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorialRequest {
    /// 解説(HTML)。Markdown で保存するときは空文字列。
    pub html: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

impl EditorialRequest {
    pub fn new(statement: Statement) -> anyhow::Result<Self> {
        if statement.text().trim().is_empty() {
            anyhow::bail!("解説が空です。空のまま送ると保存済みの解説が消えるので中止します。");
        }
        let (html, markdown) = statement.into_fields();
        Ok(Self { html, markdown })
    }
}

/// `POST /v1/problems/{id}/file/{which}` のレスポンス。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UploadResponse {
    /// サニタイズ後のファイル名。送った名前と違うことがある。
    #[serde(default, rename = "FileNames")]
    pub file_names: Vec<String>,
    #[serde(default, rename = "Warning")]
    pub warning: String,
}

/// `PUT /v1/submissions/{id}/solution` のリクエスト。
#[derive(Debug, Clone, Serialize)]
pub struct SolutionRequest {
    /// 想定解の説明。`delete` のときは無視される。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<bool>,
}

/// `GET /v1/languages` の要素。
#[derive(Debug, Clone, Deserialize)]
pub struct Language {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Ver")]
    pub ver: String,
    #[serde(default, rename = "Status")]
    pub status: String,
}

/// テストケースの入出力どちらを指すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Which {
    In,
    Out,
}

impl Which {
    pub fn as_str(self) -> &'static str {
        match self {
            Which::In => "in",
            Which::Out => "out",
        }
    }
}

impl std::fmt::Display for Which {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ProblemSettings {
        ProblemSettings {
            title: "タイトル".into(),
            tags: String::new(),
            level: 2.5,
            time_limit_ms: 2000,
            memory_limit: 1024,
            eps_mode: "-".into(),
            eps: "0.0".into(),
            wip: true,
            recruiting_tester: false,
            problem_type: 0,
            judge_type: 0,
            show_ans: true,
            enable_pure_judge: false,
            force_single_server_judge: false,
            allowed_langs: vec![],
        }
    }

    /// Markdown で保存するときは `html` を空文字列で送る。省略はしない
    /// (swagger の required を満たすため)。
    #[test]
    fn markdown_goes_to_markdown_with_empty_html() {
        let req =
            ProblemEditRequest::new(settings(), Statement::Markdown("# 問題".into())).unwrap();
        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json["html"], "");
        assert_eq!(json["markdown"], "# 問題");
    }

    /// HTML で保存するときは `markdown` を送らない。古い Markdown ソースが
    /// 残らないようにする。
    #[test]
    fn html_omits_markdown() {
        let req =
            ProblemEditRequest::new(settings(), Statement::Html("<p>問題</p>".into())).unwrap();
        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json["html"], "<p>問題</p>");
        assert!(json.get("markdown").is_none());
    }

    /// 空の本文を送ると保存済みの問題文が消えるので、必ず止める。
    #[test]
    fn empty_statement_is_rejected() {
        assert!(ProblemEditRequest::new(settings(), Statement::Markdown("  \n".into())).is_err());
        assert!(EditorialRequest::new(Statement::Html(String::new())).is_err());
    }

    /// 読み取り専用キーは送らない。未知のフィールドはサーバでエラーになる。
    #[test]
    fn read_only_keys_are_not_sent() {
        let req = ProblemEditRequest::new(settings(), Statement::Markdown("本文".into())).unwrap();
        let json = serde_json::to_value(&req).unwrap();
        for key in ["problemId", "content", "isMarkdown", "showable"] {
            assert!(json.get(key).is_none(), "{key} を送ってはいけない");
        }
        // 設定は flatten されて同じ階層に出る。
        assert_eq!(json["timeLimitMs"], 2000);
    }

    /// ジャッジコードのコンパイル状態は `WJ` → `Judge` → `AC` と遷移する。
    /// 途中の `Judge` を確定扱いすると、コンパイルが通っているのに失敗として
    /// 報告してしまう (実際にそうなった)。
    #[test]
    fn only_ac_and_ce_are_final_judge_statuses() {
        assert!(judge_status_is_final("AC"));
        assert!(judge_status_is_final("CE"));
        for in_progress in ["WJ", "Judge", "", "Compiling"] {
            assert!(
                !judge_status_is_final(in_progress),
                "{in_progress} を確定扱いしてはいけない"
            );
        }
    }

    /// API は eps を文字列でも数値でも返す。
    #[test]
    fn eps_accepts_string_or_number() {
        let from_string: ProblemEditContent =
            serde_json::from_str(&sample_get_json(r#""0.001""#)).unwrap();
        assert_eq!(from_string.settings.eps, "0.001");
        let from_number: ProblemEditContent =
            serde_json::from_str(&sample_get_json("0.001")).unwrap();
        assert_eq!(from_number.settings.eps, "0.001");
    }

    /// problem.toml は API と同じキー名で往復できる。
    #[test]
    fn settings_round_trip_through_toml() {
        let text = toml::to_string_pretty(&settings()).unwrap();
        assert!(text.contains("timeLimitMs = 2000"), "{text}");
        let parsed: ProblemSettings = toml::from_str(&text).unwrap();
        assert_eq!(parsed, settings());
    }

    fn sample_get_json(eps: &str) -> String {
        format!(
            r#"{{"problemId":1,"title":"t","tags":"","level":1,"timeLimitMs":2000,
               "memoryLimit":1024,"epsMode":"-","eps":{eps},"wip":true,
               "recruitingTester":false,"problemType":0,"judgeType":0,"showAns":true,
               "enablePureJudge":false,"forceSingleServerJudge":false,"allowedLangs":[],
               "content":"本文","isMarkdown":true,"showable":false}}"#
        )
    }
}

mod de {
    use serde::{Deserialize, Deserializer};

    /// `eps` は API が文字列で返すこともあれば数値で返すこともある。
    pub fn string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrNumber {
            String(String),
            Float(f64),
            Int(i64),
        }

        Ok(match StringOrNumber::deserialize(deserializer)? {
            StringOrNumber::String(s) => s,
            StringOrNumber::Float(f) => f.to_string(),
            StringOrNumber::Int(i) => i.to_string(),
        })
    }
}
