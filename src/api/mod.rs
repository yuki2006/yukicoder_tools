//! yukicoder 公開 API のクライアント。
//!
//! 認証はすべて `Authorization: Bearer <token>`。トークンはアカウントの API
//! キーか、問題ごとの編集トークン (`ypt_...`)。

pub mod models;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::{multipart, Client, RequestBuilder, Response};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use models::{
    EditorialContent, EditorialRequest, GeneratorContent, GeneratorRequest, JudgeCodeContent,
    JudgeCodeRequest, JudgeCodeSaveResponse, Language, ProblemEditContent, ProblemEditRequest,
    SaveResponse, SolutionRequest, UploadResponse, Which,
};

pub const DEFAULT_BASE_URL: &str = "https://yukicoder.me/api";

const USER_AGENT: &str = concat!("yukicoder-tools/", env!("CARGO_PKG_VERSION"));

/// yukicoder API クライアント。
///
/// `token` は絶対にログへ出さない。エラーメッセージにも含めない。
pub struct YukicoderClient {
    http: Client,
    base_url: String,
    token: String,
}

impl YukicoderClient {
    pub fn new(token: String, base_url: impl Into<String>) -> Result<Self> {
        if token.trim().is_empty() {
            bail!("トークンが空です");
        }
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("HTTP クライアントを作成できませんでした")?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
        })
    }

    /// 認証不要の API (`/v1/languages` など) 用。トークンを要求しない。
    pub fn anonymous(base_url: impl Into<String>) -> Result<Self> {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .context("HTTP クライアントを作成できませんでした")?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: String::new(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn authed(&self, req: RequestBuilder) -> RequestBuilder {
        req.bearer_auth(&self.token)
    }

    /// 成功以外を、本文つきのエラーに変換する。
    fn check(res: Response, what: &str) -> Result<Response> {
        let status = res.status();
        if status.is_success() {
            return Ok(res);
        }
        let body = res.text().unwrap_or_default();
        let body = body.trim();
        let hint = match status {
            StatusCode::UNAUTHORIZED => {
                "\nヒント: トークンが無い・不正・失効、または別の問題のトークンです。"
            }
            StatusCode::FORBIDDEN => {
                "\nヒント: 編集権限が無いか、入力の検証エラーです (本文を確認してください)。"
            }
            StatusCode::NOT_FOUND => "\nヒント: 問題 ID かファイル名を確認してください。",
            _ => "",
        };
        bail!(
            "{what} に失敗しました (HTTP {}){hint}{}",
            status.as_u16(),
            if body.is_empty() {
                String::new()
            } else {
                format!("\nレスポンス: {body}")
            }
        );
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str, what: &str) -> Result<T> {
        let res = self
            .authed(self.http.get(self.url(path)))
            .send()
            .with_context(|| format!("{what} のリクエストを送信できませんでした"))?;
        let body = Self::check(res, what)?
            .text()
            .with_context(|| format!("{what} のレスポンスを読めませんでした"))?;
        serde_json::from_str(&body)
            .with_context(|| format!("{what} のレスポンスを解釈できませんでした: {body}"))
    }

    fn send_json<B: Serialize>(&self, method: Method, path: &str, body: &B) -> Result<Response> {
        self.authed(self.http.request(method, self.url(path)))
            .json(body)
            .send()
            .map_err(Into::into)
    }

    /// 書き込み系 API を呼ぶ。
    ///
    /// 書き込みはすべて PUT。以前は POST だったが、サーバ側から POST の経路は
    /// 削除されている (実測で 404)。
    fn put_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        what: &str,
    ) -> Result<T> {
        let res = self
            .send_json(Method::PUT, path, body)
            .with_context(|| format!("{what} のリクエストを送信できませんでした"))?;
        Self::parse_json(res, what)
    }

    fn parse_json<T: DeserializeOwned>(res: Response, what: &str) -> Result<T> {
        let body = Self::check(res, what)?
            .text()
            .with_context(|| format!("{what} のレスポンスを読めませんでした"))?;
        // 保存系は JSON を返すが、空ボディで 200 を返す経路もある。
        if body.trim().is_empty() {
            return serde_json::from_str("{}")
                .with_context(|| format!("{what} の空レスポンスを解釈できませんでした"));
        }
        serde_json::from_str(&body)
            .with_context(|| format!("{what} のレスポンスを解釈できませんでした: {body}"))
    }

    // ---- 問題本体 -------------------------------------------------------

    pub fn get_problem_edit(&self, problem_id: i64) -> Result<ProblemEditContent> {
        self.get_json(
            &format!("/v1/problems/{problem_id}/edit"),
            "問題の取得 (GET /problems/{id}/edit)",
        )
    }

    pub fn save_problem_edit(
        &self,
        problem_id: i64,
        req: &ProblemEditRequest,
    ) -> Result<SaveResponse> {
        self.put_json(
            &format!("/v1/problems/{problem_id}/edit"),
            req,
            "問題の保存 (PUT /problems/{id}/edit)",
        )
    }

    // ---- ジェネレータ ---------------------------------------------------

    pub fn get_generator(&self, problem_id: i64) -> Result<GeneratorContent> {
        self.get_json(
            &format!("/v1/problems/{problem_id}/generator"),
            "ジェネレータの取得",
        )
    }

    pub fn save_generator(&self, problem_id: i64, req: &GeneratorRequest) -> Result<SaveResponse> {
        self.put_json(
            &format!("/v1/problems/{problem_id}/generator"),
            req,
            "ジェネレータの保存",
        )
    }

    // ---- ジャッジコード (スペシャルジャッジ) -----------------------------

    /// ジャッジコードを取得する。
    ///
    /// この API はサーバへの反映が済むまで存在しない。まだ生えていない
    /// (404) ときは `None` を返し、他の同期を止めない。
    pub fn get_judge_code(&self, problem_id: i64) -> Result<Option<JudgeCodeContent>> {
        let path = format!("/v1/problems/{problem_id}/code");
        let res = self
            .authed(self.http.get(self.url(&path)))
            .send()
            .context("ジャッジコードの取得リクエストを送信できませんでした")?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::parse_json(res, "ジャッジコードの取得").map(Some)
    }

    pub fn save_judge_code(
        &self,
        problem_id: i64,
        req: &JudgeCodeRequest,
    ) -> Result<JudgeCodeSaveResponse> {
        self.put_json(
            &format!("/v1/problems/{problem_id}/code"),
            req,
            "ジャッジコードの保存",
        )
    }

    // ---- 解説 -----------------------------------------------------------

    pub fn get_editorial(&self, problem_id: i64) -> Result<EditorialContent> {
        self.get_json(
            &format!("/v1/problems/{problem_id}/editorial"),
            "解説の取得",
        )
    }

    pub fn save_editorial(&self, problem_id: i64, req: &EditorialRequest) -> Result<SaveResponse> {
        self.put_json(
            &format!("/v1/problems/{problem_id}/editorial"),
            req,
            "解説の保存",
        )
    }

    // ---- テストケース ---------------------------------------------------

    pub fn list_testcases(&self, problem_id: i64, which: Which) -> Result<Vec<String>> {
        self.get_json(
            &format!("/v1/problems/{problem_id}/file/{which}"),
            "テストケース一覧の取得",
        )
    }

    /// テストケース 1 件の中身を取得する。本文はそのままのテキスト。
    pub fn get_testcase(&self, problem_id: i64, which: Which, name: &str) -> Result<String> {
        let path = format!("/v1/problems/{problem_id}/file/{which}/{name}");
        let res = self
            .authed(self.http.get(self.url(&path)))
            .send()
            .context("テストケースの取得リクエストを送信できませんでした")?;
        Self::check(res, "テストケースの取得")?
            .text()
            .context("テストケースの本文を読めませんでした")
    }

    /// テストケースをまとめてアップロードする。
    ///
    /// サーバ側のファイル名はアップロードするファイル名で決まる。フォームの
    /// フィールド名は `newfiles` (小文字)。
    ///
    /// swagger には 1 件ずつ送る `/file/{which}/{FileName}` もあるが、実際には
    /// そのルートは存在せず 404 になるので、こちらだけを使う。
    pub fn upload_testcases(
        &self,
        problem_id: i64,
        which: Which,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<UploadResponse> {
        let mut form = multipart::Form::new();
        for (name, content) in files {
            let part = multipart::Part::bytes(content)
                .file_name(name)
                .mime_str("text/plain")
                .context("multipart の作成に失敗しました")?;
            form = form.part("newfiles", part);
        }
        let path = format!("/v1/problems/{problem_id}/file/{which}");
        let res = self
            .authed(self.http.post(self.url(&path)))
            .multipart(form)
            .send()
            .context("テストケースのアップロードを送信できませんでした")?;
        let body = Self::check(res, "テストケースのアップロード")?
            .text()
            .context("アップロードのレスポンスを読めませんでした")?;
        serde_json::from_str(&body)
            .with_context(|| format!("アップロードのレスポンスを解釈できませんでした: {body}"))
    }

    pub fn delete_testcase(&self, problem_id: i64, which: Which, name: &str) -> Result<()> {
        let path = format!("/v1/problems/{problem_id}/file/{which}/{name}");
        let res = self
            .authed(self.http.delete(self.url(&path)))
            .send()
            .context("テストケースの削除リクエストを送信できませんでした")?;
        Self::check(res, "テストケースの削除")?;
        Ok(())
    }

    // ---- 提出 -----------------------------------------------------------

    /// 想定解などを提出する。レスポンスは JSON とは限らないので生の文字列で返す。
    pub fn submit(&self, problem_id: i64, lang: &str, source: String) -> Result<String> {
        let form = multipart::Form::new()
            .text("lang", lang.to_string())
            .text("source", source);
        let path = format!("/v1/problems/{problem_id}/submit");
        let res = self
            .authed(self.http.post(self.url(&path)))
            .multipart(form)
            .send()
            .context("提出リクエストを送信できませんでした")?;
        Self::check(res, "提出")?
            .text()
            .context("提出のレスポンスを読めませんでした")
    }

    /// AC した提出を解説ページの「想定解」一覧に登録する / 登録を消す。
    pub fn set_solution(&self, submission_id: i64, req: &SolutionRequest) -> Result<SaveResponse> {
        self.put_json(
            &format!("/v1/submissions/{submission_id}/solution"),
            req,
            "想定解の登録",
        )
    }

    // ---- その他 ---------------------------------------------------------

    /// 言語一覧。認証不要。
    pub fn languages(&self) -> Result<Vec<Language>> {
        let res = self
            .http
            .get(self.url("/v1/languages"))
            .send()
            .context("言語一覧のリクエストを送信できませんでした")?;
        let body = Self::check(res, "言語一覧の取得")?
            .text()
            .context("言語一覧のレスポンスを読めませんでした")?;
        serde_json::from_str(&body).map_err(|e| anyhow!("言語一覧を解釈できませんでした: {e}"))
    }
}
