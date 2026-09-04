//! yukicoder 公開 API のクライアント。
//!
//! 認証はすべて `Authorization: Bearer <token>`。トークンはアカウントの API
//! キーか、問題ごとの編集トークン (`ypt_...`)。

pub mod body;
pub mod models;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use models::{
    EditorialContent, EditorialRequest, GeneratorContent, GeneratorRequest, JudgeCodeContent,
    JudgeCodeRequest, JudgeCodeSaveResponse, Language, ProblemEditContent, ProblemEditRequest,
    SaveResponse, SolutionRequest, UploadResponse, ValidatorContent, ValidatorRequest, Which,
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
            // 例: ジャッジコードの削除でサーバがファイルを消せなかった場合。
            // サーバ側は登録を消さずにエラーを返すので、そのまま再実行できる。
            s if s.is_server_error() => {
                "\nヒント: サーバ側のエラーです。保存や削除は行われていない可能性があります。\
                 時間をおいて同じコマンドを実行し直してください。"
            }
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
        let bytes = serde_json::to_vec(body).context("リクエストを JSON にできませんでした")?;
        let body = body::Body::new(bytes)?;
        self.send_body(method, path, "application/json", body)
    }

    /// ボディを送る。圧縮したときは `Content-Encoding: gzip` を付ける。
    fn send_body(
        &self,
        method: Method,
        path: &str,
        content_type: &str,
        body: body::Body,
    ) -> Result<Response> {
        let mut req = self
            .authed(self.http.request(method, self.url(path)))
            .header(CONTENT_TYPE, content_type)
            .body(body.bytes);
        if body.gzipped {
            req = req.header(CONTENT_ENCODING, "gzip");
        }
        req.send().map_err(Into::into)
    }

    /// 書き込み系 API を呼ぶ。
    ///
    /// 書き込みはすべて PUT。
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
    /// この API を持たないサーバでは 404 が返る。その場合は `None` を返し、
    /// ほかの同期を止めない。
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

    // ---- validator ------------------------------------------------------

    /// validator を取得する。
    ///
    /// ジャッジコードと同じく、この API を持たないサーバでは 404 が返るので
    /// `None` を返してほかの同期を止めない。
    pub fn get_validator(&self, problem_id: i64) -> Result<Option<ValidatorContent>> {
        let path = format!("/v1/problems/{problem_id}/validator");
        let res = self
            .authed(self.http.get(self.url(&path)))
            .send()
            .context("validator の取得リクエストを送信できませんでした")?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::parse_json(res, "validator の取得").map(Some)
    }

    /// validator を保存する。レスポンスはジャッジコードの保存と同じ形。
    pub fn save_validator(
        &self,
        problem_id: i64,
        req: &ValidatorRequest,
    ) -> Result<JudgeCodeSaveResponse> {
        self.put_json(
            &format!("/v1/problems/{problem_id}/validator"),
            req,
            "validator の保存",
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
        let names: Vec<String> = self.get_json(
            &format!("/v1/problems/{problem_id}/file/{which}"),
            "テストケース一覧の取得",
        )?;
        // 名前はローカルパスに join し、URL のパスにも埋め込む。サーバ由来でも
        // 検証してから使う ('/' や '..' が混ざると外に書いてしまう)。
        for name in &names {
            if name.is_empty() || crate::local::sanitized_file_name(name) != *name {
                bail!("サーバが想定外のテストケース名を返しました: {name:?}");
            }
        }
        Ok(names)
    }

    /// テストケース 1 件の中身を、保存されているバイト列のまま取得する。
    pub fn get_testcase(&self, problem_id: i64, which: Which, name: &str) -> Result<Vec<u8>> {
        let path = format!("/v1/problems/{problem_id}/file/{which}/{name}");
        let res = self
            .authed(self.http.get(self.url(&path)))
            .send()
            .context("テストケースの取得リクエストを送信できませんでした")?;
        Ok(Self::check(res, "テストケースの取得")?
            .bytes()
            .context("テストケースの本文を読めませんでした")?
            .to_vec())
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
        let parts: Vec<body::Part> = files
            .into_iter()
            .map(|(name, content)| body::Part::file("newfiles", name, content))
            .collect();
        let (boundary, raw) = body::multipart(&parts)?;
        let path = format!("/v1/problems/{problem_id}/file/{which}");
        let res = self
            .send_body(
                Method::POST,
                &path,
                &format!("multipart/form-data; boundary={boundary}"),
                body::Body::new(raw)?,
            )
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
        let parts = [
            body::Part::text("lang", lang),
            body::Part::text("source", source),
        ];
        let (boundary, raw) = body::multipart(&parts)?;
        let path = format!("/v1/problems/{problem_id}/submit");
        let res = self
            .send_body(
                Method::POST,
                &path,
                &format!("multipart/form-data; boundary={boundary}"),
                body::Body::new(raw)?,
            )
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
