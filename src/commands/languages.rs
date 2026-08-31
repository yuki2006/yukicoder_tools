//! `yuki languages` — 提出・ジェネレータで使う言語 ID の一覧。認証不要。

use anyhow::Result;

use crate::api::{YukicoderClient, DEFAULT_BASE_URL};
use crate::config::Repo;

pub fn run(include_disabled: bool) -> Result<()> {
    // リポジトリの外でも使えるように、設定が無ければ既定の URL を使う。
    let base_url = Repo::discover()
        .map(|repo| repo.config.base_url)
        .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let client = YukicoderClient::anonymous(base_url)?;

    for lang in client.languages()? {
        let enabled = lang.status.is_empty() || lang.status == "enable";
        if !enabled && !include_disabled {
            continue;
        }
        let suffix = if enabled {
            String::new()
        } else {
            format!(" [{}]", lang.status)
        };
        println!("{:<24} {} ({}){suffix}", lang.id, lang.name, lang.ver);
    }
    Ok(())
}
