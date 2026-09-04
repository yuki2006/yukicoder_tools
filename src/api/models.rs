//! `https://yukicoder.me/api/swagger.yaml` に対応するデータ型。
//!
//! フィールド名は API と同じ camelCase を使う。`problem.toml` などのローカル
//! ファイルも同じ名前で保存し、API のドキュメントをそのまま参照できるようにする。

use std::collections::HashSet;

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
    /// `-` / `abs` / `rel` / `all` ([`EPS_MODE_LABELS`])
    pub eps_mode: String,
    /// 許容誤差。API は文字列でも数値でも返すので文字列として保持する。
    #[serde(deserialize_with = "de::string_or_number")]
    pub eps: String,
    pub wip: bool,
    pub recruiting_tester: bool,
    /// 数値コード ([`PROBLEM_TYPE_LABELS`])
    pub problem_type: i64,
    /// 数値コード ([`JUDGE_TYPE_LABELS`])
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

/// `problemType` の数値コードの意味 (WebUI の表記)。
///
/// 値は数値のまま扱い、problem.toml のコメントと diff の表示にだけ使う。
pub const PROBLEM_TYPE_LABELS: &[(i64, &str)] = &[
    (0, "通常"),
    (1, "教育的"),
    (2, "スコア"),
    (3, "ネタ"),
    (4, "未証明"),
    (5, "数学要素が高い"),
    (6, "ショートコード"),
];

/// `judgeType` の数値コードの意味。
pub const JUDGE_TYPE_LABELS: &[(i64, &str)] =
    &[(0, "通常"), (1, "スペシャル"), (2, "リアクティブ")];

/// `epsMode` の値の意味。
pub const EPS_MODE_LABELS: &[(&str, &str)] = &[
    ("-", "なし"),
    ("abs", "絶対誤差"),
    ("rel", "相対誤差"),
    ("all", "両方"),
];

pub fn problem_type_label(code: i64) -> Option<&'static str> {
    PROBLEM_TYPE_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, label)| *label)
}

pub fn judge_type_label(code: i64) -> Option<&'static str> {
    JUDGE_TYPE_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, label)| *label)
}

pub fn eps_mode_label(code: &str) -> Option<&'static str> {
    EPS_MODE_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, label)| *label)
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

/// `GET /v1/problems/{id}/validator` のレスポンス。
///
/// テストケースを検証する validator。未登録なら DB に行が無く、全フィールドが
/// 空 (数値は 0) で返る。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorContent {
    #[serde(default)]
    pub lang_id: String,
    #[serde(default)]
    pub source: String,
    /// 検証状態。ジャッジコードと違い、通常ジャッジとして走るので提出と同じ
    /// ステータス一式が出る。語彙と分類は `GET /v1/statuses` が正で、
    /// `category == "judging"` の間は実行中・実行待ち ([`judging_ids`])。
    #[serde(default)]
    pub status: String,
    /// 最後に検証が終わった時刻 (unix ナノ秒)。未登録・未実行なら 0。
    #[serde(default)]
    pub latest_check: i64,
    /// テストケースが最後に更新された時刻 (unix ナノ秒)。一度も更新して
    /// いなければ 0。テストケースの更新と同時に (再検証より先に) 進み、
    /// `status` も同時に `Pending` になる。再検証は 10 秒のデバウンスの後に走る。
    #[serde(default)]
    pub test_case_latest: i64,
}

impl ValidatorContent {
    /// 今のテストケースに対する検証結果が出ているか。
    ///
    /// 「status が judging (実行中・実行待ち) でない」ことに加えて、検証時刻が
    /// テストケース更新時刻より後であることも見る。サーバはテストケース更新と
    /// 同時に `Pending` を立てるので通常は status だけで判定できるが、status と
    /// タイムスタンプの更新の間に挟まるポーリングや、`Pending` を立てない更新
    /// 経路が将来できた場合への保険としてタイムスタンプも併用する。
    ///
    /// 比較は `>=` ではなく `>`。サーバのタイムスタンプは秒精度なので、同一
    /// 秒内に「テストケース更新 → 前回の検証完了の記録」が入ると `=` になり
    /// 得る。その場合も再検証は必ず後から走って `latest_check` を進める。
    ///
    /// `judging` は `/v1/statuses` から作った非終端 ID の集合 ([`judging_ids`])。
    /// 語彙の列挙はクライアントに持たない (増えることがあり、実際に `Pending`
    /// が増えた)。空文字列は未登録。
    pub fn is_up_to_date(&self, judging: &HashSet<String>) -> bool {
        !self.status.is_empty()
            && !judging.contains(self.status.as_str())
            && self.latest_check > self.test_case_latest
    }
}

/// `PUT /v1/problems/{id}/validator` のリクエスト。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorRequest {
    pub lang_id: String,
    /// 空文字列で削除。
    pub source: String,
}

/// `GET /v1/statuses` の要素。ジャッジステータスの一覧 (認証不要)。
///
/// レスポンスには `description` (日本語の説明) もあるが、本ツールでは使わない
/// ので読まない。
#[derive(Debug, Clone, Deserialize)]
pub struct StatusInfo {
    pub id: String,
    /// `success` / `wrong` / `judging` / `danger`。
    ///
    /// `judging` は実行中・実行待ちで、この間はポーリングを続ける。judging
    /// 以外になっても「結果が確定した」わけではない (リジャッジや、IE などが
    /// サーバ再起動で投げ直されることで、後から変わることがある)。
    pub category: String,
}

/// 「実行中・実行待ち」の分類名。
pub const STATUS_CATEGORY_JUDGING: &str = "judging";

/// ステータス一覧から、非終端 (judging) の ID 集合を作る。
///
/// ステータス ID は増えることがあるので列挙をクライアントに持たせず、
/// サーバの分類を正とする。
pub fn judging_ids(statuses: &[StatusInfo]) -> HashSet<String> {
    statuses
        .iter()
        .filter(|s| s.category == STATUS_CATEGORY_JUDGING)
        .map(|s| s.id.clone())
        .collect()
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

    /// validator の完了判定。非終端かどうかは `/v1/statuses` の judging 分類を
    /// 正とし、語彙の列挙はクライアントに持たない (増えることがあり、実際に
    /// Pending が増えた)。
    ///
    /// 完了には「status が judging でない」だけでなく、検証時刻がテストケース
    /// 更新時刻より後であることも要求する (前回の終端値を今回の結果と取り
    /// 違えないための保険)。タイムスタンプは秒精度なので、同時刻 (`=`) も
    /// 未完了として扱う。
    #[test]
    fn validator_result_must_be_newer_than_the_testcases() {
        let statuses = [
            ("WJ", "judging"),
            ("Pending", "judging"),
            ("NewWait", "judging"), // 将来増えた非終端もサーバの分類だけで追従する
            ("AC", "success"),
            ("WA", "wrong"),
        ]
        .map(|(id, category)| StatusInfo {
            id: id.into(),
            category: category.into(),
        });
        let judging = judging_ids(&statuses);
        let validator = |status: &str, latest_check: i64, test_case_latest: i64| ValidatorContent {
            status: status.into(),
            latest_check,
            test_case_latest,
            ..Default::default()
        };

        assert!(validator("AC", 200, 100).is_up_to_date(&judging));
        assert!(
            !validator("AC", 100, 200).is_up_to_date(&judging),
            "テストケースの方が新しい間は、前回の AC で完了にしない"
        );
        assert!(
            !validator("AC", 100, 100).is_up_to_date(&judging),
            "同時刻は未完了"
        );
        for in_progress in ["WJ", "Pending", "NewWait"] {
            assert!(
                !validator(in_progress, 200, 100).is_up_to_date(&judging),
                "{in_progress} は実行中・実行待ち"
            );
        }
        assert!(
            validator("WA", 200, 100).is_up_to_date(&judging),
            "失敗の終端も「結果が出た」ではある"
        );
        assert!(
            validator("AC", 100, 0).is_up_to_date(&judging),
            "テストケース未更新 (NULL=0) でも結果があれば完了"
        );
        assert!(!validator("", 200, 100).is_up_to_date(&judging), "未登録");
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

    /// 数値コードに名前を引ける。problem.toml のコメントと diff の表示に使う。
    #[test]
    fn code_labels_are_available() {
        assert_eq!(judge_type_label(1), Some("スペシャル"));
        assert_eq!(problem_type_label(6), Some("ショートコード"));
        assert_eq!(eps_mode_label("abs"), Some("絶対誤差"));
        assert_eq!(judge_type_label(9), None, "未知のコードは名前なし");
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
