//! AI生成コード置き場 (`/users/{UserId}/ai_code`) の API。
//!
//! 読むのは認証不要。書くのはアカウントの API キーか、`scope=ai_code` 付きで
//! 受け取った提出専用トークン (`ysb_...`) で、対象は自分の投稿だけ。
//! 削除の API は無い (間違えて消さないよう、サイトから行う)。

use anyhow::{Context, Result};
use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::YukicoderClient;

/// 置き場の 1 件 (swagger の `UserAICode`)。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCode {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub lang: String,
    /// コードのバイト数。
    pub size: i64,
    /// サーバが記録した更新日時 (RFC 3339)。
    pub updated_at: String,
    /// コード。1 件の取得のときだけ入る。
    #[serde(default)]
    pub source: Option<String>,
}

/// 投稿 (POST) と差し替え (PUT) の本文。3 つとも要る。
#[derive(Debug, Clone, Serialize)]
pub struct AiCodeInput {
    pub title: String,
    pub lang: String,
    pub source: String,
}

/// タイトル・言語の変更 (PATCH) の本文。送らなかった方は今の値のまま。
#[derive(Debug, Clone, Serialize)]
pub struct AiCodeLabel {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

impl YukicoderClient {
    /// 自分の投稿の一覧 (本文なし、更新日時の新しい順)。
    pub fn my_ai_codes(&self) -> Result<Vec<AiCode>> {
        self.get_json("/v1/ai_code", "AI生成コードの一覧の取得")
    }

    /// あるユーザーの投稿の一覧 (本文なし)。認証不要なのでトークンを付けない。
    pub fn user_ai_codes(&self, user_id: i64) -> Result<Vec<AiCode>> {
        let res = self
            .http
            .get(self.url(&format!("/v1/users/{user_id}/ai_code")))
            .send()
            .context("AI生成コードの一覧のリクエストを送信できませんでした")?;
        Self::parse_json(res, "AI生成コードの一覧の取得")
    }

    /// 1 件を本文ごと取得する。認証不要なのでトークンを付けない。
    pub fn user_ai_code(&self, user_id: i64, id: i64) -> Result<AiCode> {
        let res = self
            .http
            .get(self.url(&format!("/v1/users/{user_id}/ai_code/{id}")))
            .send()
            .context("AI生成コードのリクエストを送信できませんでした")?;
        Self::parse_json(res, "AI生成コードの取得")
    }

    /// 投稿する。返るのは本文を除いた 1 件。
    pub fn create_ai_code(&self, input: &AiCodeInput) -> Result<AiCode> {
        self.write_json(Method::POST, "/v1/ai_code", input, "AI生成コードの投稿")
    }

    /// タイトル・言語・コードを差し替える。更新日時は今になる。
    pub fn update_ai_code(&self, id: i64, input: &AiCodeInput) -> Result<AiCode> {
        self.write_json(
            Method::PUT,
            &format!("/v1/ai_code/{id}"),
            input,
            "AI生成コードの差し替え",
        )
    }

    /// タイトルと言語だけを変える。更新日時は変わらない。
    pub fn label_ai_code(&self, id: i64, label: &AiCodeLabel) -> Result<AiCode> {
        self.write_json(
            Method::PATCH,
            &format!("/v1/ai_code/{id}"),
            label,
            "AI生成コードのタイトル・言語の変更",
        )
    }
}
