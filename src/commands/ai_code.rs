//! `yuki-tool ai-code` — AI生成コード置き場 (`/users/{UserId}/ai_code`) の操作。
//!
//! 生成 AI で作ったコードを、コンテストより前に公開しておくための置き場。
//! 読むのは認証不要。書くのは `yuki-tool login --ai-code` のログインか、
//! アカウントの API キー。削除の API は無いので、サイトから行ってもらう。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};

use crate::api::ai_code::{AiCode, AiCodeInput, AiCodeLabel};
use crate::api::{YukicoderClient, DEFAULT_BASE_URL, SITE_URL};
use crate::commands::login::{self, Saved};
use crate::config::{self, Repo};
use crate::credentials::SCOPE_AI_CODE;
use crate::local::{display_path, read_text, write_text};

const LOGIN_COMMAND: &str = "yuki-tool login --ai-code";

/// 書き込みと「自分の」読み取りに使う認証つきのクライアント。
struct Session {
    client: YukicoderClient,
    /// 保存したログインを使っているときの、その保存先。API キーなら `None`。
    login_path: Option<PathBuf>,
}

impl Session {
    /// 認証を決める。
    ///
    /// 1. `yuki-tool login --ai-code` で保存したログイン (期限内で、権限があるもの)
    /// 2. アカウントの API キー (`YUKICODER_API_KEY`。環境変数か `.env`)
    ///
    /// 権限の狭いログインを先に使う。どちらも無ければ、ログインをやり直す
    /// コマンドを案内して止まる。
    fn open() -> Result<Self> {
        let (path, saved) = login::saved()?;
        if let Saved::Valid(login) = &saved {
            if login.has_scope(SCOPE_AI_CODE) {
                return Ok(Self {
                    client: YukicoderClient::new(login.access_token.clone(), DEFAULT_BASE_URL)?,
                    login_path: Some(path),
                });
            }
        }
        if let Some(key) = config::find_api_key(&Repo::discover()?.root)? {
            return Ok(Self {
                client: YukicoderClient::new(key, DEFAULT_BASE_URL)?,
                login_path: None,
            });
        }
        match saved {
            Saved::Valid(_) => bail!(
                "保存しているログインには AI生成コード置き場の権限がありません。\
                 `{LOGIN_COMMAND}` で受け取り直してください。"
            ),
            Saved::Expired => {
                bail!("ログインの期限が切れています。`{LOGIN_COMMAND}` をやり直してください。")
            }
            Saved::Missing => bail!(
                "AI生成コード置き場を操作するには、`{LOGIN_COMMAND}` でログインするか、\
                 アカウントの API キーを {} (環境変数か {}) に設定してください。",
                config::API_KEY,
                config::DOTENV_FILE
            ),
        }
    }

    /// API を呼ぶ。保存したログインが 401 なら、消してやり直しを案内する。
    fn call<T>(&self, f: impl FnOnce(&YukicoderClient) -> Result<T>) -> Result<T> {
        f(&self.client).map_err(|err| match &self.login_path {
            Some(path) => login::relogin_on_unauthorized(err, path, LOGIN_COMMAND),
            None => err,
        })
    }
}

/// 画面の URL。ID は数値なので、そのまま組み立てる。
fn page_url(user_id: i64, id: i64) -> String {
    format!("{SITE_URL}/users/{user_id}/ai_code/{id}")
}

pub fn list(user: Option<i64>) -> Result<()> {
    let codes = match user {
        Some(user_id) => YukicoderClient::anonymous(DEFAULT_BASE_URL)?.user_ai_codes(user_id)?,
        None => Session::open()?.call(|client| client.my_ai_codes())?,
    };
    let Some(first) = codes.first() else {
        println!("投稿はありません。");
        return Ok(());
    };
    for code in &codes {
        println!(
            "{}\t{}\t{}\t{} バイト\t{}",
            code.id, code.updated_at, code.lang, code.size, code.title
        );
    }
    println!("置き場のページ: {SITE_URL}/users/{}/ai_code", first.user_id);
    Ok(())
}

pub fn show(id: i64, user: Option<i64>, output: Option<&Path>) -> Result<()> {
    let user_id = match user {
        Some(user_id) => user_id,
        // 自分の投稿。読むのに認証は要らないが、自分のユーザー ID を知るのに使う。
        None => Session::open()?.call(|client| client.auth_user())?.id,
    };
    let code = YukicoderClient::anonymous(DEFAULT_BASE_URL)?.user_ai_code(user_id, id)?;
    let source = code
        .source
        .as_deref()
        .context("サーバがコードの本文を返しませんでした")?;
    let summary = describe(&code);
    match output {
        Some(path) => {
            write_text(path, source)?;
            println!("{summary}\n{} に書き出しました。", display_path(path));
        }
        // 標準出力は本文だけにする (リダイレクトでそのままファイルにできるように)。
        None => {
            eprintln!("{summary}");
            print!("{source}");
        }
    }
    Ok(())
}

pub fn post(title: String, lang: String, file: &Path) -> Result<()> {
    let source = read_source(file)?;
    let session = Session::open()?;
    let code = session.call(|client| {
        client.create_ai_code(&AiCodeInput {
            title,
            lang,
            source,
        })
    })?;
    println!("投稿しました。\n{}", describe(&code));
    Ok(())
}

pub fn update(id: i64, file: &Path, title: Option<String>, lang: Option<String>) -> Result<()> {
    let source = read_source(file)?;
    let session = Session::open()?;
    // 差し替え (PUT) はタイトル・言語も送る必要がある。省略された方は今の値を使う。
    let (title, lang) = match (title, lang) {
        (Some(title), Some(lang)) => (title, lang),
        (title, lang) => {
            let current = session
                .call(|client| client.my_ai_codes())?
                .into_iter()
                .find(|code| code.id == id)
                .with_context(|| {
                    format!("自分の投稿に ID {id} がありません (`yuki-tool ai-code list` で確認できます)")
                })?;
            (title.unwrap_or(current.title), lang.unwrap_or(current.lang))
        }
    };
    let code = session.call(|client| {
        client.update_ai_code(
            id,
            &AiCodeInput {
                title,
                lang,
                source,
            },
        )
    })?;
    println!(
        "差し替えました (更新日時が新しくなりました)。\n{}",
        describe(&code)
    );
    Ok(())
}

pub fn label(id: i64, title: Option<String>, lang: Option<String>) -> Result<()> {
    if title.is_none() && lang.is_none() {
        bail!("--title か --lang を指定してください");
    }
    let session = Session::open()?;
    let code = session.call(|client| client.label_ai_code(id, &AiCodeLabel { title, lang }))?;
    println!(
        "タイトル・言語を変えました (更新日時は変わりません)。\n{}",
        describe(&code)
    );
    Ok(())
}

/// 削除の API は無い。何も消していないので、エラーとして案内を出す。
pub fn delete() -> Result<()> {
    bail!(
        "削除は API ではできません。間違えないようにサイトから行ってください。\n\
         `yuki-tool ai-code list` の最後に出る置き場のページから、該当のコードを開いて削除します。"
    )
}

fn read_source(file: &Path) -> Result<String> {
    let source = read_text(file)?;
    if source.trim().is_empty() {
        bail!("{} が空です", display_path(file));
    }
    Ok(source)
}

fn describe(code: &AiCode) -> String {
    format!(
        "ID {}: {} ({}, {} バイト, 更新 {})\n{}",
        code.id,
        code.title,
        code.lang,
        code.size,
        code.updated_at,
        page_url(code.user_id, code.id)
    )
}
