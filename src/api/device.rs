//! CLI ログイン (デバイス認可、RFC 8628) と、ログイン中のユーザーの確認。
//!
//! 手順は <https://yukicoder.me/help/cli> が正。ブラウザで許可してもらい、
//! 提出専用トークン (`ysb_...`) を受け取る。`device_code` とトークンは秘密なので、
//! それらが入ったレスポンスの本文はエラーメッセージに出さない。

use anyhow::{anyhow, bail, Context, Result};
use reqwest::StatusCode;
use serde::Deserialize;

use super::YukicoderClient;

const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// `POST /v1/device/code` のレスポンス。
///
/// `device_code` を持つので `Debug` は付けない (うっかり表示しないため)。
#[derive(Clone, Deserialize)]
pub struct DeviceCode {
    /// CLI だけが持つ。利用者には見せない。
    pub device_code: String,
    /// 利用者がブラウザで確かめるコード。
    pub user_code: String,
    pub verification_uri: String,
    /// `user_code` が入った状態で開く URL。
    pub verification_uri_complete: String,
    /// この秒数のうちに許可してもらう。
    pub expires_in: u64,
    /// トークンを取りに行く間隔 (秒)。
    pub interval: u64,
}

/// `POST /v1/device/token` が許可後に返すトークン。`Debug` は付けない。
#[derive(Clone, Deserialize)]
pub struct DeviceToken {
    pub access_token: String,
    /// 有効期間 (秒)。
    pub expires_in: u64,
    /// トークンでできること (空白区切り)。`submit` か `submit ai_code`。
    pub scope: String,
}

/// `POST /v1/device/token` を 1 回呼んだ結果。
pub enum DevicePoll {
    Granted(DeviceToken),
    /// まだ許可されていない。同じ間隔で続ける。
    Pending,
    /// 呼ぶ間隔が短すぎる。間隔を延ばして続ける。
    SlowDown,
}

/// RFC 8628 3.5 / RFC 6749 5.2 のエラー形式。
#[derive(Deserialize)]
struct DeviceError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// `GET /v1/authUser` のレスポンス。読むのは表示に使うフィールドだけ。
#[derive(Debug, Clone, Deserialize)]
pub struct AuthUser {
    #[serde(rename = "Id")]
    pub id: i64,
    #[serde(rename = "Name")]
    pub name: String,
}

impl YukicoderClient {
    /// デバイス認可を始める。認証は要らない。
    ///
    /// `scope` は提出のほかに求める権限 (`ai_code`)。要らないときは送らない。
    pub fn device_code(&self, client_name: &str, scope: Option<&str>) -> Result<DeviceCode> {
        const WHAT: &str = "ログインの開始 (POST /device/code)";
        let mut form = vec![("client_name", client_name)];
        if let Some(scope) = scope {
            form.push(("scope", scope));
        }
        let res = self
            .http
            .post(self.url("/v1/device/code"))
            .form(&form)
            .send()
            .with_context(|| format!("{WHAT} のリクエストを送信できませんでした"))?;
        let body = Self::check(res, WHAT)?
            .text()
            .with_context(|| format!("{WHAT} のレスポンスを読めませんでした"))?;
        // 本文には device_code が入っているので、エラーに含めない。
        serde_json::from_str(&body)
            .map_err(|e| anyhow!("{WHAT} のレスポンスを解釈できませんでした: {e}"))
    }

    /// トークンを取りに行く。認証は要らない。
    ///
    /// 許可待ちと間隔の指示は [`DevicePoll`] で返し、それ以外の失敗 (拒否・
    /// 期限切れなど) はエラーにする。
    pub fn device_token(&self, device_code: &str) -> Result<DevicePoll> {
        const WHAT: &str = "トークンの受け取り (POST /device/token)";
        let res = self
            .http
            .post(self.url("/v1/device/token"))
            .form(&[("grant_type", GRANT_TYPE), ("device_code", device_code)])
            .send()
            .with_context(|| format!("{WHAT} のリクエストを送信できませんでした"))?;
        // 許可待ちなどは 400 で返る。それ以外の失敗は通常のエラーにする。
        let res = if res.status() == StatusCode::BAD_REQUEST {
            res
        } else {
            Self::check(res, WHAT)?
        };
        let granted = res.status().is_success();
        let body = res
            .text()
            .with_context(|| format!("{WHAT} のレスポンスを読めませんでした"))?;
        if granted {
            // 本文にはトークンが入っているので、エラーに含めない。
            let token: DeviceToken = serde_json::from_str(&body)
                .map_err(|e| anyhow!("{WHAT} のレスポンスを解釈できませんでした: {e}"))?;
            return Ok(DevicePoll::Granted(token));
        }
        let err: DeviceError = serde_json::from_str(&body)
            .with_context(|| format!("{WHAT} のエラーを解釈できませんでした: {body}"))?;
        match err.error.as_str() {
            "authorization_pending" => Ok(DevicePoll::Pending),
            "slow_down" => Ok(DevicePoll::SlowDown),
            "access_denied" => bail!("ブラウザで拒否されたので、ログインをやめました。"),
            "expired_token" => bail!(
                "時間内に許可されませんでした。もう一度 `yuki-tool login` を実行してください。"
            ),
            other => bail!(
                "{WHAT} に失敗しました ({other}){}",
                if err.error_description.is_empty() {
                    String::new()
                } else {
                    format!(": {}", err.error_description)
                }
            ),
        }
    }

    /// トークンの持ち主を確かめる。
    pub fn auth_user(&self) -> Result<AuthUser> {
        self.get_json("/v1/authUser", "ログイン中のユーザーの取得")
    }
}
