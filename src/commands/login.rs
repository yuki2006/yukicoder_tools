//! `yuki-tool login` / `logout` / `whoami` — ブラウザで許可して受け取る
//! 提出専用トークン (`ysb_...`) の管理。
//!
//! 流れは OAuth 2.0 のデバイス認可 (RFC 8628)。手順は
//! <https://yukicoder.me/help/cli> が正。トークンは 24 時間で切れ、提出と
//! 本人確認にしか使えない (`--ai-code` で受け取れば AI生成コード置き場にも使える)。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};

use crate::api::device::{DeviceCode, DevicePoll, DeviceToken};
use crate::api::{self, YukicoderClient, DEFAULT_BASE_URL, SITE_URL};
use crate::credentials::{self, Login, SCOPE_AI_CODE};

/// 許可の画面に出す、この CLI の名前。
const CLIENT_NAME: &str = "yuki-tool";

/// 保存しているログインの状態。
pub enum Saved {
    Missing,
    Expired,
    Valid(Login),
}

/// 保存先と、保存しているログインの状態を返す。
pub fn saved() -> Result<(PathBuf, Saved)> {
    let path = credentials::default_path()?;
    let state = match credentials::load(&path)? {
        None => Saved::Missing,
        Some(login) if login.is_expired(credentials::now()?) => Saved::Expired,
        Some(login) => Saved::Valid(login),
    };
    Ok((path, state))
}

/// 保存したログインで呼んだ API が 401 なら、ログインを消してやり直しを案内する。
///
/// 期限と権限は呼ぶ前に手元で確かめているので、ここに来るのは利用者が
/// サイトで失効させたときなど。使えないトークンを残すと次も同じ失敗になる。
pub fn relogin_on_unauthorized(err: anyhow::Error, path: &Path, command: &str) -> anyhow::Error {
    if !api::is_unauthorized(&err) {
        return err;
    }
    match credentials::remove(path) {
        Ok(_) => err.context(format!(
            "保存していたログインが使えなくなっていたので消しました (失効したか、期限が\
             切れています)。`{command}` をやり直してください"
        )),
        Err(remove_err) => err.context(format!(
            "保存していたログインが使えません (失効したか、期限が切れています)。\
             `{command}` をやり直してください。古いログインは消せませんでした: {remove_err:#}"
        )),
    }
}

pub fn run(ai_code: bool, no_browser: bool) -> Result<()> {
    // 許可してもらった後で保存に失敗しないよう、保存先を先に決める。
    let path = credentials::default_path()?;
    let client = YukicoderClient::anonymous(DEFAULT_BASE_URL)?;

    // 要らない権限は求めない。AI生成コード置き場は --ai-code のときだけ。
    let code = client.device_code(CLIENT_NAME, ai_code.then_some(SCOPE_AI_CODE))?;
    check_verification_uri(&code)?;

    println!(
        "ブラウザで次の URL を開き、表示されるコードが下と同じことを確かめて「許可する」を押してください。\n\
         \n  {}\n  コード: {}\n\n\
         (コードを手で入力するなら {} を開きます)",
        code.verification_uri_complete, code.user_code, code.verification_uri
    );
    if ai_code {
        println!("提出に加えて、AI生成コード置き場の操作 (一覧・投稿・差し替え) を求めます。");
    }
    if !no_browser {
        if let Err(err) = open_browser(&code.verification_uri_complete) {
            println!("ブラウザを開けませんでした ({err})。上の URL を手で開いてください。");
        }
    }
    println!("許可を待っています... ({} 分以内)", code.expires_in / 60);

    let token = wait_for_token(&client, &code, std::thread::sleep)?;
    let login = Login {
        expires_at: credentials::now()? + token.expires_in,
        scope: token.scope,
        access_token: token.access_token,
    };
    credentials::save(&path, &login)?;
    println!("ログインを保存しました: {}", path.display());

    if ai_code && !login.has_scope(SCOPE_AI_CODE) {
        println!(
            "注意: AI生成コード置き場の権限は付きませんでした (受け取った権限: {})。",
            login.scope
        );
    }
    let user = YukicoderClient::new(login.access_token.clone(), DEFAULT_BASE_URL)?.auth_user()?;
    print_status(&user.name, user.id, &login)?;
    println!(
        "失効させるときは {SITE_URL}/device を開いてください (`yuki-tool logout` は手元の保存を消すだけです)。"
    );
    Ok(())
}

pub fn logout() -> Result<()> {
    let path = credentials::default_path()?;
    if credentials::remove(&path)? {
        println!(
            "保存していたログインを消しました: {}\n\
             トークン自体は期限まで有効です。失効させるなら {SITE_URL}/device を開いてください。",
            path.display()
        );
    } else {
        println!("ログインは保存されていません。");
    }
    Ok(())
}

pub fn whoami() -> Result<()> {
    let (path, state) = saved()?;
    let login = match state {
        Saved::Valid(login) => login,
        Saved::Expired => {
            bail!("ログインの期限が切れています。`yuki-tool login` をやり直してください。")
        }
        Saved::Missing => bail!("ログインしていません。`yuki-tool login` を実行してください。"),
    };
    let user = YukicoderClient::new(login.access_token.clone(), DEFAULT_BASE_URL)?
        .auth_user()
        .map_err(|err| relogin_on_unauthorized(err, &path, "yuki-tool login"))?;
    print_status(&user.name, user.id, &login)
}

fn print_status(name: &str, user_id: i64, login: &Login) -> Result<()> {
    println!(
        "{name} (ユーザー ID {user_id}) としてログインしています。\n権限: {} / 期限: {}",
        login.scope,
        login.remaining(credentials::now()?)
    );
    Ok(())
}

/// サーバが返した URL が yukicoder の許可ページかを確かめる。
///
/// この URL は利用者に見せ、ブラウザにも渡す。サーバ由来でも検証してから使う。
fn check_verification_uri(code: &DeviceCode) -> Result<()> {
    let page = format!("{SITE_URL}/device");
    let complete_ok = code
        .verification_uri_complete
        .strip_prefix(&page)
        .is_some_and(|rest| rest.starts_with('?'));
    if code.verification_uri != page || !complete_ok {
        bail!(
            "サーバが想定外の許可ページを返しました: {:?} / {:?}",
            code.verification_uri,
            code.verification_uri_complete
        );
    }
    Ok(())
}

/// 許可されるまでトークンを取りに行く。
///
/// 待った時間は眠った秒数の合計で数える (`sleep` を差し替えて試せるように)。
fn wait_for_token(
    client: &YukicoderClient,
    code: &DeviceCode,
    mut sleep: impl FnMut(Duration),
) -> Result<DeviceToken> {
    // 0 秒が来ても、間を空けずに呼び続けることはしない。
    let mut interval = code.interval.max(1);
    let mut waited = 0;
    loop {
        sleep(Duration::from_secs(interval));
        waited += interval;
        match client.device_token(&code.device_code)? {
            DevicePoll::Granted(token) => return Ok(token),
            DevicePoll::Pending => {}
            // RFC 8628 3.5: 以後の間隔を 5 秒延ばす。
            DevicePoll::SlowDown => interval += 5,
        }
        if waited >= code.expires_in {
            bail!("時間内に許可されませんでした。もう一度 `yuki-tool login` を実行してください。");
        }
    }
}

/// 既定のブラウザで URL を開く。シェルを通さず、URL は 1 つの引数として渡す。
fn open_browser(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};

    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::serve_sequence;

    fn device_code(interval: u64, expires_in: u64) -> DeviceCode {
        DeviceCode {
            device_code: "ydc_dummy".into(),
            user_code: "WDJB-MJHT".into(),
            verification_uri: format!("{SITE_URL}/device"),
            verification_uri_complete: format!("{SITE_URL}/device?user_code=WDJB-MJHT"),
            expires_in,
            interval,
        }
    }

    /// 許可待ちは同じ間隔で続け、slow_down で 5 秒延ばし、許可されたら受け取る。
    #[test]
    fn polls_until_granted() {
        let (base_url, server) = serve_sequence(vec![
            (
                400,
                r#"{"error":"authorization_pending","error_description":"まだ"}"#,
            ),
            (400, r#"{"error":"slow_down","error_description":"早い"}"#),
            (
                400,
                r#"{"error":"authorization_pending","error_description":"まだ"}"#,
            ),
            (
                200,
                r#"{"access_token":"ysb_dummy","token_type":"Bearer","expires_in":86400,"scope":"submit ai_code"}"#,
            ),
        ]);
        let client = YukicoderClient::anonymous(base_url).unwrap();
        let mut slept = Vec::new();

        let token = match wait_for_token(&client, &device_code(5, 600), |d| slept.push(d.as_secs()))
        {
            Ok(token) => token,
            Err(err) => panic!("{err:#}"),
        };

        assert_eq!(token.access_token, "ysb_dummy");
        assert_eq!(token.scope, "submit ai_code");
        assert_eq!(token.expires_in, 86400);
        assert_eq!(slept, [5, 5, 10, 10]);

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 4);
        for request in &requests {
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/v1/device/token");
            assert_eq!(
                request.authorization, None,
                "ログインの API に認証は付けない"
            );
            assert_eq!(
                request.body,
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=ydc_dummy"
            );
        }
    }

    /// 拒否されたら、そこでやめる (続けて取りに行かない)。
    #[test]
    fn stops_when_denied() {
        let (base_url, server) = serve_sequence(vec![(
            400,
            r#"{"error":"access_denied","error_description":"拒否されました"}"#,
        )]);
        let client = YukicoderClient::anonymous(base_url).unwrap();

        let err = match wait_for_token(&client, &device_code(5, 600), |_| {}) {
            Ok(_) => panic!("拒否されたのにトークンを受け取った"),
            Err(err) => format!("{err:#}"),
        };

        assert!(err.contains("拒否"), "{err}");
        assert!(!err.contains("ydc_dummy"), "device_code を出さない: {err}");
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// サーバが期限切れを返さなくても、expires_in を過ぎたらやめる。
    #[test]
    fn gives_up_after_expires_in() {
        let pending = r#"{"error":"authorization_pending","error_description":"まだ"}"#;
        let (base_url, server) = serve_sequence(vec![(400, pending), (400, pending)]);
        let client = YukicoderClient::anonymous(base_url).unwrap();

        let result = wait_for_token(&client, &device_code(5, 10), |_| {});

        assert!(result.is_err());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// 許可ページが yukicoder のものでなければ、表示もブラウザ起動もしない。
    #[test]
    fn rejects_foreign_verification_uri() {
        assert!(check_verification_uri(&device_code(5, 600)).is_ok());

        let mut foreign = device_code(5, 600);
        foreign.verification_uri_complete = "https://example.com/device?user_code=X".into();
        assert!(check_verification_uri(&foreign).is_err());

        let mut lookalike = device_code(5, 600);
        lookalike.verification_uri_complete = format!("{SITE_URL}/device.example.com/?user_code=X");
        assert!(check_verification_uri(&lookalike).is_err());
    }

    /// 401 のときだけ保存したログインを消す。ほかの失敗では残す。
    #[test]
    fn unauthorized_removes_the_saved_login() {
        let dir = std::env::temp_dir().join(format!(
            "yukicoder-tools-test-relogin-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("login.json");
        let login = Login {
            access_token: "ysb_dummy".into(),
            scope: "submit".into(),
            expires_at: u64::MAX,
        };
        credentials::save(&path, &login).unwrap();

        let (base_url, server) = serve_sequence(vec![(403, r#"{"error":"だめ"}"#)]);
        let client = YukicoderClient::new("ysb_dummy".into(), base_url).unwrap();
        let err =
            relogin_on_unauthorized(client.auth_user().unwrap_err(), &path, "yuki-tool login");
        server.join().unwrap();
        assert!(!format!("{err:#}").contains("やり直して"), "{err:#}");
        assert!(path.is_file(), "403 では消さない");

        let (base_url, server) = serve_sequence(vec![(401, r#"{"error":"失効"}"#)]);
        let client = YukicoderClient::new("ysb_dummy".into(), base_url).unwrap();
        let err =
            relogin_on_unauthorized(client.auth_user().unwrap_err(), &path, "yuki-tool login");
        server.join().unwrap();
        let message = format!("{err:#}");
        assert!(
            message.contains("`yuki-tool login` をやり直して"),
            "{message}"
        );
        assert!(!message.contains("ysb_dummy"), "{message}");
        assert!(!path.exists(), "401 なら消す");
    }
}
