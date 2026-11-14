//! `yuki-tool login` で受け取った提出専用トークン (`ysb_...`) の保存。
//!
//! ログインは利用者ごとのものなので、リポジトリではなく利用者の設定
//! ディレクトリに置く。本人だけが読めるファイルにし、トークンは表示もログ
//! 出力もしない (`Login` に `Debug` を付けないのはそのため)。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// AI生成コード置き場の権限。デバイス認可の `scope` に付ける値。
pub const SCOPE_AI_CODE: &str = "ai_code";

/// 保存しているログイン。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Login {
    pub access_token: String,
    /// トークンでできること (空白区切り)。`submit` か `submit ai_code`。
    pub scope: String,
    /// 期限 (UNIX 秒)。受け取った時刻に `expires_in` を足したもの。
    pub expires_at: u64,
}

impl Login {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scope.split_whitespace().any(|s| s == scope)
    }

    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    /// 期限までの残りを「あと 23 時間 59 分」の形にする。
    pub fn remaining(&self, now: u64) -> String {
        let minutes = self.expires_at.saturating_sub(now) / 60;
        format!("あと {} 時間 {} 分", minutes / 60, minutes % 60)
    }
}

/// 今の時刻 (UNIX 秒)。
pub fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("システムの時刻が 1970 年より前です")?
        .as_secs())
}

/// ログインを保存するファイル。
///
/// Windows は `%APPDATA%\yuki-tool\login.json`、それ以外は XDG の規約どおり
/// `$XDG_CONFIG_HOME/yuki-tool/login.json` (無ければ `$HOME/.config/...`)。
pub fn default_path() -> Result<PathBuf> {
    Ok(config_dir(|key| std::env::var(key).ok())?
        .join("yuki-tool")
        .join("login.json"))
}

#[cfg(windows)]
fn config_dir(env: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    match env("APPDATA").filter(|v| !v.trim().is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => bail!("ログインの保存先を決められません (環境変数 APPDATA がありません)"),
    }
}

#[cfg(not(windows))]
fn config_dir(env: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    if let Some(dir) = env("XDG_CONFIG_HOME").filter(|v| !v.trim().is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    match env("HOME").filter(|v| !v.trim().is_empty()) {
        Some(home) => Ok(PathBuf::from(home).join(".config")),
        None => bail!(
            "ログインの保存先を決められません (環境変数 XDG_CONFIG_HOME も HOME もありません)"
        ),
    }
}

/// 保存したログインを読む。ファイルが無ければ `None`。
pub fn load(path: &Path) -> Result<Option<Login>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("{} を読めませんでした", path.display()));
        }
    };
    // 中身にはトークンが入っているので、エラーに含めない。
    let login: Login = serde_json::from_str(&text).map_err(|_| {
        anyhow::anyhow!(
            "{} を解釈できませんでした。`yuki-tool login` をやり直してください。",
            path.display()
        )
    })?;
    Ok(Some(login))
}

/// ログインを保存する。Unix では本人だけが読めるパーミッション (600) にする。
pub fn save(path: &Path, login: &Login) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("{} を作成できませんでした", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(login).context("ログインを JSON にできませんでした")?;
    let mut file =
        open_private(path).with_context(|| format!("{} を書けませんでした", path.display()))?;
    file.write_all(&json)
        .and_then(|()| file.write_all(b"\n"))
        .with_context(|| format!("{} を書けませんでした", path.display()))
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // mode は新しく作るときにしか効かない。前からあるファイルも 600 に揃える。
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Windows では `%APPDATA%` 以下が利用者ごとの領域で、ファイルはその ACL を継ぐ。
#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

/// 保存したログインを消す。消したら true、もともと無ければ false。
pub fn remove(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("{} を消せませんでした", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "yukicoder-tools-test-login-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir.join("nested").join("login.json")
    }

    fn login(scope: &str) -> Login {
        Login {
            access_token: "ysb_dummy".into(),
            scope: scope.into(),
            expires_at: 1_000,
        }
    }

    /// 保存して読み直せる。親ディレクトリは無ければ作る。消した後は無い扱い。
    #[test]
    fn save_load_remove_roundtrip() {
        let path = temp_file("roundtrip");
        assert!(load(&path).unwrap().is_none());

        save(&path, &login("submit ai_code")).unwrap();
        assert!(load(&path).unwrap().unwrap() == login("submit ai_code"));

        // 上書きすると前の内容は残らない (短い内容で上書きしても壊れない)。
        save(&path, &login("submit")).unwrap();
        assert!(load(&path).unwrap().unwrap() == login("submit"));

        assert!(remove(&path).unwrap());
        assert!(!remove(&path).unwrap());
        assert!(load(&path).unwrap().is_none());
    }

    /// 本人だけが読める。前からあった緩いパーミッションのファイルも揃える。
    #[cfg(unix)]
    #[test]
    fn saved_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_file("private");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        save(&path, &login("submit")).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// 壊れたファイルのエラーに、中身 (トークン) を出さない。
    #[test]
    fn broken_file_does_not_leak_its_content() {
        let path = temp_file("broken");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"accessToken":"ysb_secret","scope":"#).unwrap();

        let err = match load(&path) {
            Ok(_) => panic!("壊れたファイルを読めてしまった"),
            Err(err) => format!("{err:#}"),
        };

        assert!(!err.contains("ysb_secret"), "{err}");
    }

    #[test]
    fn scope_and_expiry() {
        let both = login("submit ai_code");
        assert!(both.has_scope(SCOPE_AI_CODE));
        assert!(!login("submit").has_scope(SCOPE_AI_CODE));
        // 部分一致では通さない。
        assert!(!login("submit ai_code_extra").has_scope(SCOPE_AI_CODE));

        assert!(!both.is_expired(999));
        assert!(both.is_expired(1_000));
        let later = Login {
            expires_at: 10_000,
            ..both
        };
        assert_eq!(later.remaining(10_000 - 3_660), "あと 1 時間 1 分");
        // 期限を過ぎていても負にならない。
        assert_eq!(later.remaining(20_000), "あと 0 時間 0 分");
    }

    #[cfg(windows)]
    #[test]
    fn config_dir_needs_appdata() {
        assert!(config_dir(|_| None).is_err());
        let dir = config_dir(|key| (key == "APPDATA").then(|| "C:\\x".to_string())).unwrap();
        assert_eq!(dir, PathBuf::from("C:\\x"));
    }

    #[cfg(not(windows))]
    #[test]
    fn config_dir_follows_xdg() {
        assert!(config_dir(|_| None).is_err());
        let home = |key: &str| (key == "HOME").then(|| "/home/u".to_string());
        assert_eq!(config_dir(home).unwrap(), PathBuf::from("/home/u/.config"));
        let xdg = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some("/cfg".to_string()),
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(config_dir(xdg).unwrap(), PathBuf::from("/cfg"));
    }
}
